//! Rendered resize capture + controlled stationary controller ticks + real history.
use super::super::{EditorView, musical_time::SnapGrid, viewport::Viewport};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, ClipId, Command, Created, TrackId};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = Instant::now() + Duration::from_secs(5);
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
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
    midi: bool,
    viewport: Option<Viewport>,
) -> (gpui::AnyWindowHandle, Entity<EditorView>, TrackId, ClipId) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    let (event, track, clip) = if midi {
        service
            .submit(Request::Edit(Command::AddMidiTrack {
                name: "midi".into(),
            }))
            .unwrap();
        let event = next(&service);
        let track = event.state.project.tracks()[0].id();
        service
            .submit(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_raw(4 << 32),
                length: BeatTime::from_raw(4 << 32),
            }))
            .unwrap();
        let event = next(&service);
        let Created::Clip(clip) = event.created.unwrap() else {
            panic!("clip")
        };
        service
            .submit(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: BeatTime::from_raw(4 << 32),
                source_start: BeatTime::ZERO,
                loop_start: BeatTime::ZERO,
                loop_end: BeatTime::from_raw(4 << 32),
                looping: true,
                length: BeatTime::from_raw(4 << 32),
            }))
            .unwrap();
        (next(&service), track, clip)
    } else {
        service
            .submit(Request::Import(files.wav("resize.wav", 48_000, 96_000)))
            .unwrap();
        let event = next(&service);
        let track = event.state.project.tracks()[0].id();
        let clip = event.state.project.tracks()[0].clips()[0].id();
        service
            .submit(Request::Edit(Command::MoveClip {
                clip,
                track,
                start: 96_000,
            }))
            .unwrap();
        next(&service);
        service
            .submit(Request::Edit(Command::SetClipLooping {
                clip,
                looping: true,
            }))
            .unwrap();
        (next(&service), track, clip)
    };
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline resize test".into()))
            });
            editor.update(cx, |v, cx| {
                v.state = Some(event.state);
                v.viewport = viewport;
                v.snap_grid = SnapGrid::Off;
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, track, clip)
}
fn capture(
    visual: &mut VisualTestContext,
    clip: ClipId,
    midi: bool,
    left: bool,
) -> gpui::Point<gpui::Pixels> {
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "{}clip-trim-{}-{}",
            if midi { "midi-" } else { "" },
            if left { "left" } else { "right" },
            clip.value()
        )
        .into_boxed_str(),
    );
    let down = visual
        .debug_bounds(selector)
        .expect("rendered resize handle")
        .center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(if left { 12.0 } else { -12.0 }), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    down
}
fn held_tick(editor: &Entity<EditorView>, visual: &mut VisualTestContext, now: Instant) {
    editor.update(&mut *visual, |v, cx| {
        v.tick_clip_gesture_autoscroll(now, cx);
        v.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx);
        cx.notify();
    });
    visual.run_until_parked();
}
#[gpui::test]
fn held_resize_edges_pan_both_directions_stop_and_commit_one_history_edit(cx: &mut TestAppContext) {
    for midi in [false, true] {
        for left in [false, true] {
            let viewport = Viewport {
                start: 72_000,
                length: 192_000,
            };
            let (handle, editor, track, clip) = fixture(cx, midi, Some(viewport));
            let mut visual = VisualTestContext::from_window(handle, cx);
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            let down = capture(&mut visual, clip, midi, left);
            let edge = editor.read_with(&visual, |v, _| {
                let lane = v.drag_lanes[&track];
                point(
                    px(if left {
                        lane.left + 3.0
                    } else {
                        lane.left + lane.width - 3.0
                    }),
                    down.y,
                )
            });
            visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let now = Instant::now();
            held_tick(&editor, &mut visual, now);
            let after_view = editor.read_with(&visual, |v, _| {
                let vp = v.visible_viewport(v.state.as_ref().unwrap().timeline_frames);
                assert!(
                    if left {
                        vp.start < viewport.start
                    } else {
                        vp.start > viewport.start
                    },
                    "held edge pans midi={midi} left={left}"
                );
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                vp
            });
            let center = editor.read_with(&visual, |v, _| {
                let lane = v.drag_lanes[&track];
                point(px(lane.left + lane.width / 2.0), down.y)
            });
            visual.simulate_mouse_move(center, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            held_tick(&editor, &mut visual, now + Duration::from_millis(100));
            editor.read_with(&visual, |v, _| {
                assert_eq!(
                    v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
                    after_view,
                    "leaving edge stops scrolling"
                )
            });
            visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
            let after = editor.read_with(&visual, |v, _| {
                assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
                v.state.as_ref().unwrap().project.snapshot()
            });
            assert_ne!(after, before);
            held_tick(&editor, &mut visual, now + Duration::from_millis(200));
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
fn held_resize_can_pan_right_beyond_original_fitted_display_extent(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let (handle, editor, track, clip) = fixture(cx, midi, None);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let horizon = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().timeline_frames);
        let down = capture(&mut visual, clip, midi, false);
        let edge = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(px(lane.left + lane.width - 3.0), down.y)
        });
        visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        held_tick(&editor, &mut visual, Instant::now());
        editor.read_with(&visual, |v, _| {
            let viewport = v.visible_viewport(horizon);
            assert!(
                viewport.start > 0 && viewport.end().unwrap() > horizon,
                "fit display is not an authored content limit: midi={midi}"
            );
        });
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
    }
}

#[gpui::test]
fn resize_edge_cancel_and_stale_owners_cannot_scroll_or_commit(cx: &mut TestAppContext) {
    for midi in [false, true] {
        for stale in [false, true] {
            let (handle, editor, track, clip) = fixture(
                cx,
                midi,
                Some(Viewport {
                    start: 72_000,
                    length: 192_000,
                }),
            );
            let mut visual = VisualTestContext::from_window(handle, cx);
            let down = capture(&mut visual, clip, midi, false);
            let edge = editor.read_with(&visual, |v, _| {
                let lane = v.drag_lanes[&track];
                point(px(lane.left + lane.width - 3.0), down.y)
            });
            visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let now = Instant::now();
            held_tick(&editor, &mut visual, now);
            if stale {
                editor.update(&mut visual, |v, _| {
                    v.project_generation = v.project_generation.wrapping_add(1);
                });
            } else {
                visual.simulate_keystrokes("escape");
                visual.run_until_parked();
            }
            held_tick(&editor, &mut visual, now + Duration::from_millis(100));
            editor.read_with(&visual, |v, _| {
                assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
                assert!(v.last_enqueued_edit.is_none());
                assert!(v.trim_display_end().is_none());
            });
            let viewport = editor.read_with(&visual, |v, _| v.viewport);
            held_tick(&editor, &mut visual, now + Duration::from_secs(10));
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.viewport, viewport, "no stray timer movement")
            });
            visual.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(
                    v.last_enqueued_edit.is_none(),
                    "retired payload cannot commit"
                )
            });
        }
    }
}

#[gpui::test]
fn resize_left_pan_stops_at_zero_and_nonloop_audio_source_limit_stops_right_pan(
    cx: &mut TestAppContext,
) {
    let (handle, editor, track, clip) = fixture(
        cx,
        false,
        Some(Viewport {
            start: 1_000,
            length: 192_000,
        }),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    let down = capture(&mut visual, clip, false, true);
    let edge = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(px(lane.left + 3.0), down.y)
    });
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let now = Instant::now();
    held_tick(&editor, &mut visual, now);
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport.unwrap().start, 0));
    held_tick(&editor, &mut visual, now + Duration::from_secs(5));
    editor.read_with(&visual, |v, _| assert_eq!(v.viewport.unwrap().start, 0));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        assert!(v.enqueue(Request::Edit(Command::SetClipLooping {
            clip,
            looping: false
        })));
        v.viewport = None;
        cx.notify();
    });
    pump(&editor, &mut visual);
    let down = capture(&mut visual, clip, false, false);
    let edge = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(px(lane.left + lane.width - 3.0), down.y)
    });
    visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| {
        v.visible_viewport(v.state.as_ref().unwrap().timeline_frames)
    });
    held_tick(&editor, &mut visual, Instant::now());
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
            before,
            "no panning away from exhausted source"
        );
        let drag = v.active_trim_drag.as_ref().unwrap();
        assert_eq!(
            drag.preview.borrow().as_ref().unwrap().clip.duration(),
            96_000
        );
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
}
