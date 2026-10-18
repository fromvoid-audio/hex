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
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    TrackId,
    ClipId,
    super::super::external_audio_drop_ui_test::BatchFiles,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let browser_marker = files.wav("bounded-browser.wav", 48_000, 48);
    let directory = browser_marker.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
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
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.viewport = viewport;
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (handle, editor, track, clip, files)
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
            let (handle, editor, track, clip, _files) = fixture(cx, midi, Some(viewport));
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
        let (handle, editor, track, clip, _files) = fixture(cx, midi, None);
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
            let (handle, editor, track, clip, _files) = fixture(
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
                assert!(
                    v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none(),
                    "retire midi={midi} stale={stale}"
                );
                assert!(v.last_enqueued_edit.is_none());
                assert!(v.trim_display_end().is_none());
            });
            visual.simulate_mouse_move(
                point(edge.x - px(6.0), edge.y),
                MouseButton::Left,
                Modifiers::default(),
            );
            visual.run_until_parked();
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
    let (handle, editor, track, clip, _files) = fixture(
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

#[gpui::test]
fn resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release(cx: &mut TestAppContext) {
    for stretch in [false, true] {
        let (handle, editor, track, clip, _files) = fixture(cx, false, None);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, cx| {
            v.snap_grid = SnapGrid::default();
            cx.notify();
        });
        let down = capture(&mut visual, clip, false, false);
        let edge = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(px(lane.left + lane.width - 3.0), down.y)
        });
        visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let modifiers = Modifiers {
            platform: true,
            shift: stretch,
            ..Modifiers::default()
        };
        visual.simulate_modifiers_change(modifiers);
        visual.run_until_parked();
        let now = Instant::now();
        held_tick(&editor, &mut visual, now);
        let preview = editor.read_with(&visual, |v, _| {
            let d = v.active_trim_drag.as_ref().unwrap();
            assert_eq!(d.stretch_modifier.get(), stretch);
            assert!(d.snap_modifier.get());
            d.preview.borrow().as_ref().unwrap().clip.clone()
        });
        visual.simulate_modifiers_change(Modifiers {
            shift: stretch,
            ..Modifiers::default()
        });
        visual.run_until_parked();
        let snapped = editor.read_with(&visual, |v, _| {
            v.active_trim_drag
                .as_ref()
                .unwrap()
                .preview
                .borrow()
                .as_ref()
                .unwrap()
                .clip
                .clone()
        });
        assert_ne!(
            preview, snapped,
            "stationary Command changes snapping without losing stretch mode"
        );
        visual.simulate_modifiers_change(modifiers);
        visual.run_until_parked();
        visual.simulate_mouse_up(edge, MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.clip(clip),
                Some(&preview),
                "release matches last actual modifier preview"
            );
            assert!(v.active_trim_drag.is_none());
        });
    }
}

#[gpui::test]
fn canceled_fitted_resize_does_not_resurrect_hidden_pan_after_project_growth(
    cx: &mut TestAppContext,
) {
    check_retired_pan_then_growth(cx, false);
}
#[gpui::test]
fn stale_fitted_resize_does_not_resurrect_hidden_pan_after_project_growth(cx: &mut TestAppContext) {
    check_retired_pan_then_growth(cx, true);
}
fn check_retired_pan_then_growth(cx: &mut TestAppContext, stale: bool) {
    for midi in [false, true] {
        let (handle, editor, track, clip, _files) = fixture(cx, midi, None);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let (original_view, original_audio, original_midi) = editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            assert!(v.viewport.is_none(), "genuine fitted view");
            (
                v.visible_viewport(state.timeline_frames),
                state.project.clip(clip).cloned(),
                state.project.midi_clip(clip).cloned(),
            )
        });
        let down = capture(&mut visual, clip, midi, false);
        let edge = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(px(lane.left + lane.width - 3.0), down.y)
        });
        visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let now = Instant::now();
        held_tick(&editor, &mut visual, now);
        let owned_pan = editor.read_with(&visual, |v, _| {
            let pan = v.viewport.expect("actual edge-pan stored viewport");
            assert!(
                pan.start > 0 && pan.end().unwrap() > v.state.as_ref().unwrap().timeline_frames
            );
            pan
        });
        if stale {
            // A real independent edit changes the canonical revision while the
            // pointer gesture is still held. Its subsequent tick must retire it.
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::Gain { track, gain: 0.75 })));
            });
            pump(&editor, &mut visual);
            held_tick(&editor, &mut visual, now + Duration::from_millis(100));
        } else {
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
        }
        visual.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let stored_after_cancel = editor.read_with(&visual, |v, _| {
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
            assert!(v.trim_display_end().is_none());
            assert_eq!(
                v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
                original_view,
                "the immediate fitted clamp can conceal a stale stored offset"
            );
            v.viewport
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(48).unwrap(),
                length: BeatTime::from_quarters(4).unwrap(),
            })));
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            assert!(state.timeline_frames > owned_pan.end().unwrap());
            let visible = v.visible_viewport(state.timeline_frames);
            eprintln!("retired pan midi={midi} stale={stale} original={original_view:?} held={owned_pan:?} stored_after_cancel={stored_after_cancel:?} grown_visible={visible:?}");
            assert_eq!(state.project.clip(clip), original_audio.as_ref());
            assert_eq!(state.project.midi_clip(clip), original_midi.as_ref());
            assert_eq!(visible, original_view, "a canceled/stale resize must not reappear when unrelated content grows");
        });
    }
}

#[gpui::test]
fn inward_shift_resize_consumes_last_valid_stationary_pan_step(cx: &mut TestAppContext) {
    check_inward_last_valid_step(cx, false, false, false);
}
#[gpui::test]
fn inward_midi_resize_keeps_positive_extent_at_stationary_pan_limit(cx: &mut TestAppContext) {
    check_inward_last_valid_step(cx, true, false, false);
}
#[gpui::test]
fn inward_left_shift_resize_consumes_last_valid_stationary_pan_step(cx: &mut TestAppContext) {
    check_inward_last_valid_step(cx, false, true, false);
}
#[gpui::test]
fn inward_pan_then_supported_stretch_release_commits_once_and_undoes(cx: &mut TestAppContext) {
    for left in [false, true] {
        check_inward_last_valid_step(cx, false, left, true);
    }
}
fn check_inward_last_valid_step(cx: &mut TestAppContext, midi: bool, left: bool, supported: bool) {
    let viewport = if left {
        Viewport {
            start: 0,
            length: 188_000,
        }
    } else {
        Viewport {
            start: 100_000,
            length: 192_000,
        }
    };
    let (handle, editor, track, clip, _files) = fixture(cx, midi, Some(viewport));
    let mut visual = VisualTestContext::from_window(handle, cx);
    let before_project =
        editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = capture(&mut visual, clip, midi, left);
    let edge = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(
            px(if left {
                lane.left + lane.width - 3.0
            } else {
                lane.left + 3.0
            }),
            down.y,
        )
    });
    let modifiers = Modifiers {
        shift: !midi,
        ..Modifiers::default()
    };
    visual.simulate_mouse_move(edge, MouseButton::Left, modifiers);
    visual.run_until_parked();
    let before = editor.read_with(&visual, |v, _| {
        if !midi {
            let d = v.active_trim_drag.as_ref().unwrap();
            assert!(d.stretch_modifier.get());
            assert!(d.preview.borrow().as_ref().unwrap().clip.duration() > 1);
        }
        v.viewport.unwrap()
    });
    let now = Instant::now();
    held_tick(&editor, &mut visual, now);
    let at_limit = editor.read_with(&visual, |v, _| {
        let after = v.viewport.unwrap();
        eprintln!("inward held midi={midi} before={before:?} after={after:?}");
        assert!(
            if left {
                after.start > before.start
            } else {
                after.start < before.start
            },
            "consume valid final partial step rather than reject overshooting full tick"
        );
        if midi {
            let d = v.active_midi_trim_drag.as_ref().unwrap();
            assert!(d.preview.borrow().as_ref().unwrap().length().raw() > 0);
        } else {
            let d = v.active_trim_drag.as_ref().unwrap();
            assert_eq!(
                d.preview.borrow().as_ref().unwrap().clip.duration(),
                1,
                "last legal positive stretch interval"
            );
        }
        after
    });
    held_tick(&editor, &mut visual, now + Duration::from_secs(1));
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.viewport,
            Some(at_limit),
            "no timer drift past legal resize bound"
        )
    });
    let release = if supported {
        let target = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            let frame = if left { 168_000_u64 } else { 120_000 };
            point(
                px(lane.left
                    + ((frame - at_limit.start) as f64 / at_limit.length as f64
                        * f64::from(lane.width)) as f32),
                down.y,
            )
        });
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        target
    } else {
        edge
    };
    let desired = editor.read_with(&visual, |v, _| {
        v.active_trim_drag
            .as_ref()
            .and_then(|d| d.preview.borrow().as_ref().map(|p| p.clip.clone()))
    });
    visual.simulate_mouse_up(release, MouseButton::Left, modifiers);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        eprintln!("inward release midi={midi} left={left} supported={supported} error={:?} submitted={:?}", v.error, v.last_enqueued_edit);
        let p = &v.state.as_ref().unwrap().project;
        assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
        if midi {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(p.midi_clip(clip).unwrap().length().raw() > 0);
        } else if supported {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert_eq!(p.clip(clip), desired.as_ref());
            assert!((12_000..=96_000).contains(&p.clip(clip).unwrap().duration()));
        } else {
            // Model-positive is not a waiver of the separate prepared warp
            // capability. A 96000:1 transform must refuse atomically, visibly.
            assert!(v.error.as_ref().is_some_and(|e| e.contains("warp stretch including pitch compensation must be within 1/8..8")));
            assert_eq!(p.snapshot(), before_project);
        }
    });
    if supported {
        let after = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before_project)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn asynchronous_refused_resize_normalizes_its_hidden_pan_before_later_growth(
    cx: &mut TestAppContext,
) {
    check_actual_terminal_then_growth(cx, false, true);
}
#[gpui::test]
fn accepted_edge_short_of_view_end_does_not_reveal_raw_pan_on_later_growth(
    cx: &mut TestAppContext,
) {
    for midi in [false, true] {
        check_actual_terminal_then_growth(cx, midi, false);
    }
}
fn check_actual_terminal_then_growth(cx: &mut TestAppContext, midi: bool, refuse: bool) {
    let (handle, editor, track, clip, _files) = fixture(cx, midi, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    if !refuse {
        // Real distant content makes the edge-band time offset exceed the
        // existing two-second canonical display margin, without synthetic state.
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(168).unwrap(),
                length: BeatTime::from_quarters(4).unwrap()
            })));
        });
        pump(&editor, &mut visual);
    }
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = capture(&mut visual, clip, midi, false);
    let edge = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(
            px(lane.left + lane.width - if refuse { 3.0 } else { 23.0 }),
            down.y,
        )
    });
    let modifiers = Modifiers {
        shift: refuse,
        ..Modifiers::default()
    };
    visual.simulate_mouse_move(edge, MouseButton::Left, modifiers);
    visual.run_until_parked();
    let now = Instant::now();
    for step in 0..=if refuse { 18 } else { 1 } {
        editor.update(&mut visual, |v, cx| {
            v.tick_clip_gesture_autoscroll(now + Duration::from_millis(step * 50), cx);
            cx.notify();
        });
        visual.run_until_parked();
    }
    let owned = editor.read_with(&visual, |v, _| {
        let viewport = v.viewport.unwrap();
        assert!(
            viewport.start > 0
                && viewport.end().unwrap() > v.state.as_ref().unwrap().timeline_frames
        );
        if refuse {
            assert!(
                v.active_trim_drag
                    .as_ref()
                    .unwrap()
                    .preview
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .clip
                    .duration()
                    > 96_000 * 8
            );
        }
        viewport
    });
    visual.simulate_mouse_up(edge, MouseButton::Left, modifiers);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(
            v.last_enqueued_edit.is_some(),
            "actual accepted enqueue, not immediate admission refusal"
        )
    });
    pump(&editor, &mut visual);
    let settled = editor.read_with(&visual, |v,_| {
        let state=v.state.as_ref().unwrap();
        if refuse { assert!(v.error.is_some(), "real worker must refuse unsupported transform");assert_eq!(state.project.snapshot(),before); }
        else { assert!(v.error.is_none(),"{:?}",v.error);assert_ne!(state.project.snapshot(),before); }
        assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
        let visible=v.visible_viewport(state.timeline_frames);
        eprintln!("terminal pan midi={midi} refuse={refuse} held={owned:?} raw={:?} horizon={} settled={visible:?} error={:?}",v.viewport,state.timeline_frames,v.error);
        assert!(state.timeline_frames<owned.end().unwrap(), "endpoint intentionally short of transient display end");
        visible
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: BeatTime::from_quarters(512).unwrap(),
            length: BeatTime::from_quarters(4).unwrap()
        })));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v,_| {
        let state=v.state.as_ref().unwrap();
        assert!(state.timeline_frames>owned.end().unwrap());
        let grown=v.visible_viewport(state.timeline_frames);
        eprintln!("terminal later growth midi={midi} refuse={refuse} settled={settled:?} grown={grown:?}");
        assert_eq!(grown,settled,"actual terminal must retire hidden raw resize pan before later unrelated project growth");
    });
}
