//! Rendered resize capture + controlled stationary controller ticks + real history.
use super::super::{EditorView, musical_time::SnapGrid, viewport::Viewport};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{BeatTime, ClipId, Command, Created, TrackId};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < until, "session event did not arrive");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let until = crate::test_wait::hang_deadline();
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
            Instant::now() < until,
            "editor worker requests did not drain"
        );
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
    fixture_with_streamed_source(cx, midi, viewport, false)
}
fn fixture_with_streamed_source(
    cx: &mut TestAppContext,
    midi: bool,
    viewport: Option<Viewport>,
    streamed_source: bool,
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    TrackId,
    ClipId,
    super::super::external_audio_drop_ui_test::BatchFiles,
) {
    fixture_with_streamed_source_at_rate(cx, midi, viewport, streamed_source, 48_000)
}
fn fixture_with_streamed_source_at_rate(
    cx: &mut TestAppContext,
    midi: bool,
    viewport: Option<Viewport>,
    streamed_source: bool,
    sample_rate: u32,
) -> (
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    TrackId,
    ClipId,
    super::super::external_audio_drop_ui_test::BatchFiles,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let browser_marker = files.wav("bounded-browser.wav", sample_rate, sample_rate / 1_000);
    let directory = browser_marker.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(sample_rate, 32, 8_000_000).unwrap();
    next(&service);
    let replacement = if streamed_source {
        assert!(!midi);
        service
            .submit(Request::NewWithAudioMode {
                mode: hex_session::DocumentAudioMode::VerifiedStreaming,
                reservation: Some(hex_session::SourcePlanReservation {
                    reader: hex_media::VerifiedWavReaderLimits {
                        reader: hex_media::WavReaderLimits {
                            max_file_bytes: 64 * 1024 * 1024,
                            max_source_frames: 8_000_000,
                            max_header_count: 32,
                            max_window_frames: 4096,
                        },
                        max_manifest_bytes: 1024 * 1024,
                    },
                    max_retained_sources: 1,
                    max_path_bytes: 1024 * 1024,
                    max_manifest_bytes: 4 * 1024 * 1024,
                }),
            })
            .unwrap();
        let event = next(&service);
        assert!(event.project_replaced);
        Some(event)
    } else {
        None
    };
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
            .submit(Request::Import(files.wav(
                "resize.wav",
                sample_rate,
                sample_rate * 2,
            )))
            .unwrap();
        let event = next(&service);
        let track = event.state.project.tracks()[0].id();
        let clip = event.state.project.tracks()[0].clips()[0].id();
        service
            .submit(Request::Edit(Command::MoveClip {
                clip,
                track,
                start: u64::from(sample_rate) * 2,
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
                if let Some(replacement) = replacement {
                    v.receive(replacement);
                }
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
        // Keep the two-second musical geometry and actual Beats preparation,
        // bounding debug DSP work while testing gesture and terminal delivery.
        let (handle, editor, track, clip, _files) =
            fixture_with_streamed_source_at_rate(cx, false, None, false, 8_000);
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
            let preview = d.preview.borrow();
            let preview = preview.as_ref().unwrap();
            assert!(preview.clip.duration() > 0);
            hex_engine::audio_transform::validate_clip_warp_preview(
                &preview.clip,
                v.state.as_ref().unwrap().project.tempo_map(),
            )
            .expect("last displayed extent is admitted, not merely model-positive");
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
        v.active_trim_drag.as_ref().and_then(|d| {
            d.preview
                .borrow()
                .as_ref()
                .map(|p| (p.clip.clone(), p.command_frame_for_test()))
        })
    });
    visual.simulate_mouse_up(release, MouseButton::Left, modifiers);
    if let Some((_, frame)) = &desired {
        editor.read_with(&visual, |v, _| {
            let expected_edge = if left { hex_project::TrimEdge::Left } else { hex_project::TrimEdge::Right };
            assert!(matches!(v.last_enqueued_edit.as_ref(),
                Some(Command::StretchAudioClipEdge { clip: submitted, edge, frame: submitted_frame })
                if *submitted == clip && *edge == expected_edge && *submitted_frame == *frame
            ), "release must submit the exact retained command frame: {:?}", v.last_enqueued_edit);
        });
    }
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        eprintln!("inward release midi={midi} left={left} supported={supported} error={:?} submitted={:?}", v.error, v.last_enqueued_edit);
        let p = &v.state.as_ref().unwrap().project;
        assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
        if midi {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(p.midi_clip(clip).unwrap().length().raw() > 0);
        } else {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert_eq!(p.clip(clip), desired.as_ref().map(|(clip, _)| clip));
            hex_engine::audio_transform::validate_clip_warp_preview(
                p.clip(clip).unwrap(), p.tempo_map(),
            ).expect("real worker accepted the displayed supported extent");
            assert!(v.pending_trim.is_none());
        }
    });
    if !midi {
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
    // Verified streamed originals deliberately lack resident transform PCM.
    // Geometry admission is valid; the real worker must refuse preparation.
    let (handle, editor, track, clip, _files) =
        fixture_with_streamed_source(cx, midi, None, refuse);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let before_history = editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        (
            state.revision,
            state.can_undo,
            state.can_redo,
            state.unsaved,
        )
    });
    let down = capture(&mut visual, clip, midi, false);
    let edge = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(px(lane.left + lane.width - 3.0), down.y)
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
            let drag = v.active_trim_drag.as_ref().unwrap();
            let preview = drag.preview.borrow();
            let preview = preview.as_ref().unwrap();
            assert!(preview.clip.duration() > 0);
            hex_engine::audio_transform::validate_clip_warp_preview(
                &preview.clip,
                v.state.as_ref().unwrap().project.tempo_map(),
            )
            .expect("refusal fixture must display supported Warp geometry");
        }
        viewport
    });
    let release = if refuse {
        edge
    } else {
        // Return inside after a real pan, then make an accepted small extension.
        // Its real display horizon remains the eight-second minimum, shorter
        // than the stored transient pan. Endpoint alone is not the oracle.
        let inside = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(
                px(lane.left
                    + ((200_000 - owned.start) as f64 / owned.length as f64 * f64::from(lane.width))
                        as f32),
                down.y,
            )
        });
        visual.simulate_mouse_move(inside, MouseButton::Left, modifiers);
        visual.run_until_parked();
        inside
    };
    let retained_command_frame = refuse.then(|| {
        editor.read_with(&visual, |v, _| {
            let drag = v.active_trim_drag.as_ref().unwrap();
            drag.preview
                .borrow()
                .as_ref()
                .unwrap()
                .command_frame_for_test()
        })
    });
    visual.simulate_mouse_up(release, MouseButton::Left, modifiers);
    if let Some(frame) = retained_command_frame {
        editor.read_with(&visual, |v, _| {
            assert!(matches!(v.last_enqueued_edit.as_ref(),
                Some(Command::StretchAudioClipEdge { clip: submitted, edge: hex_project::TrimEdge::Right, frame: submitted_frame })
                if *submitted == clip && *submitted_frame == frame
            ));
            assert!(v.pending_trim.is_some(), "own accepted command retains PendingTrim until its actual worker terminal");
            assert_eq!(v.worker_requests_pending, 1, "actual own FIFO submission");
        });
    }
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
        if refuse { assert!(v.error.as_ref().is_some_and(|error| error.contains("preparation currently require resident audio; streamed source changes were not committed")), "actual streamed preparation refusal, not range validation: {:?}", v.error);assert_eq!(state.project.snapshot(),before);assert_eq!((state.revision, state.can_undo, state.can_redo, state.unsaved),before_history);assert!(v.pending_trim.is_none()); }
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

#[gpui::test]
fn fitted_resize_release_keeps_accepted_pan_horizon_until_exact_worker_terminal(
    cx: &mut TestAppContext,
) {
    for midi in [false, true] {
        let (handle, editor, track, clip, _files) = fixture(cx, midi, None);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = capture(&mut visual, clip, midi, false);
        let edge = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(px(lane.left + lane.width - 3.0), down.y)
        });
        visual.simulate_mouse_move(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        held_tick(&editor, &mut visual, Instant::now());
        let viewport = editor.read_with(&visual, |v, _| {
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames)
        });
        let selector = if midi {
            "midi-trim-live-preview"
        } else {
            "trim-live-preview"
        };
        let held = visual.debug_bounds(selector).unwrap();
        visual.simulate_mouse_up(edge, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds(selector).unwrap(),
            held,
            "released fitted preview midi={midi}"
        );
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_trim.is_some());
            assert_eq!(
                v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
                viewport
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_trim.is_none());
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
            let state = v.state.as_ref().unwrap();
            assert_eq!(
                v.visible_viewport(state.timeline_frames),
                viewport,
                "canonical accepted viewport midi={midi}"
            );
            assert_ne!(state.project.snapshot(), before);
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn partial_clamped_audio_source_pan_does_not_overshoot_applied_edge(cx: &mut TestAppContext) {
    check_partial_clamped_pan(cx, false);
}
#[gpui::test]
fn partial_clamped_midi_minimum_pan_does_not_overshoot_applied_edge(cx: &mut TestAppContext) {
    check_partial_clamped_pan(cx, true);
}
fn check_partial_clamped_pan(cx: &mut TestAppContext, midi: bool) {
    let viewport = if midi {
        Viewport {
            start: 100_000,
            length: 192_000,
        }
    } else {
        Viewport {
            start: 0,
            length: 188_000,
        }
    };
    let (handle, editor, track, clip, _files) = fixture(cx, midi, Some(viewport));
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    if !midi {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetClipLooping {
                clip,
                looping: false
            })));
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Right,
                frame: 186_000
            })));
        });
        pump(&editor, &mut visual);
    }
    let original = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let down = capture(&mut visual, clip, midi, false);
    let pointer = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(
            px(if midi {
                lane.left + 3.0
            } else {
                lane.left + lane.width - 3.0
            }),
            down.y,
        )
    });
    visual.simulate_mouse_move(pointer, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview_edge = |v: &EditorView| {
        if midi {
            let d = v.active_midi_trim_drag.as_ref().unwrap();
            let p = d.preview.borrow().unwrap();
            v.state
                .as_ref()
                .unwrap()
                .project
                .tempo_map()
                .beat_to_frame(BeatTime::from_raw(p.start().raw() + p.length().raw()))
                .unwrap()
        } else {
            let d = v.active_trim_drag.as_ref().unwrap();
            let p = d.preview.borrow();
            let p = &p.as_ref().unwrap().clip;
            p.start() + p.duration()
        }
    };
    let before_edge = editor.read_with(&visual, |v, _| preview_edge(v));
    let before_view = editor.read_with(&visual, |v, _| v.viewport.unwrap());
    let boundary = if midi { 96_000 } else { 192_000 };
    let remaining = before_edge.abs_diff(boundary);
    let requested = (before_view.length as f64 * 1.5 * 0.05).round() as u64;
    assert!(
        remaining > 0 && remaining < requested,
        "actual model boundary lies within one held step: midi={midi} edge={before_edge} remaining={remaining} step={requested}"
    );
    let selector = if midi {
        "midi-trim-live-preview"
    } else {
        "trim-live-preview"
    };
    let before_bounds = visual.debug_bounds(selector).unwrap();
    let now = Instant::now();
    held_tick(&editor, &mut visual, now);
    let after_edge = editor.read_with(&visual, |v, _| preview_edge(v));
    let after_view = editor.read_with(&visual, |v, _| v.viewport.unwrap());
    let pan = after_view.start.abs_diff(before_view.start);
    let applied = after_edge.abs_diff(before_edge);
    eprintln!(
        "partial clamped actual midi={midi} before={before_edge} after={after_edge} viewport={before_view:?}->{after_view:?} applied={applied} pan={pan} requested={requested}"
    );
    assert!(pan > 0 && applied > 0);
    assert!(
        pan.abs_diff(applied) <= 1,
        "viewport may consume only actual applied edge change (one frame rounding tolerance)"
    );
    assert_eq!(after_edge, boundary);
    if midi {
        // The canonical minimum is one Q32 beat unit. Both endpoints project
        // to the same integer frame, so existing track painting omits it. This
        // checks the actual representation, not a fabricated visible rectangle;
        // the pan-distance and real release/Undo assertions still apply.
        editor.read_with(&visual, |v, _| {
            let p = v.active_midi_trim_drag.as_ref().unwrap().preview.borrow();
            let p = p.as_ref().unwrap();
            assert_eq!(p.length().raw(), 1);
            let tempo = v.state.as_ref().unwrap().project.tempo_map();
            assert_eq!(tempo.beat_to_frame(p.start()).unwrap(), boundary);
            assert_eq!(
                tempo
                    .beat_to_frame(BeatTime::from_raw(p.start().raw() + p.length().raw()))
                    .unwrap(),
                boundary
            );
        });
        assert!(visual.debug_bounds(selector).is_none());
    } else {
        let after_bounds = visual.debug_bounds(selector).unwrap();
        assert!(
            (f32::from(before_bounds.right()) - f32::from(after_bounds.right())).abs() <= 4.0,
            "stationary edge stays aligned (existing3px minimum visual width plus1px rounding)"
        );
    }
    held_tick(&editor, &mut visual, now + Duration::from_secs(1));
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.viewport,
            Some(after_view),
            "no drift after legal partial step"
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original);
    });
    visual.simulate_mouse_up(pointer, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let project = &v.state.as_ref().unwrap().project;
        if midi {
            assert_eq!(project.midi_clip(clip).unwrap().length().raw(), 1);
        } else {
            let clip = project.clip(clip).unwrap();
            assert_eq!(clip.start() + clip.duration(), boundary);
        }
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), original)
    });
}

#[gpui::test]
fn audio_event_outside_release_only_owns_focus_during_its_actual_gesture(cx: &mut TestAppContext) {
    let (handle, editor, track, clip, _files) = fixture(cx, false, None);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_click(header.center(), Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetClipLooping {
            clip,
            looping: false
        })));
    });
    pump(&editor, &mut visual);
    let body = visual
        .debug_bounds("audio-events-waveform")
        .expect("actual lower event editor");
    let outside = editor.read_with(&visual, |v, _| {
        let lane = v.drag_lanes[&track];
        point(
            px(lane.left + lane.width / 2.0),
            px(lane.top + lane.height / 2.0),
        )
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    // No event-editor gesture: a release in Arrangement belongs to Arrangement.
    visual.update(|window, cx| editor.read(cx).focus.focus(window));
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        let v = editor.read(cx);
        assert!(v.audio_event_editor.gesture.is_none());
        assert!(v.focus.is_focused(window));
        assert_eq!(v.worker_requests_pending, 0);
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
    let down = point(
        body.origin.x + body.size.width * 0.35,
        body.center().y + px(10.0),
    );
    let moved = point(down.x + body.size.width * 0.2, down.y);
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        let v = editor.read(cx);
        assert!(
            v.audio_event_editor.gesture.is_some(),
            "actual event pointer capture"
        );
        assert!(v.audio_event_focus.is_focused(window));
        assert_eq!(v.worker_requests_pending, 0, "preview submits no edit");
    });
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        let v = editor.read(cx);
        assert!(
            v.audio_event_editor.gesture.is_none(),
            "outside release retires own preview"
        );
        assert!(
            v.audio_event_focus.is_focused(window),
            "active outside release retains event focus"
        );
        assert_eq!(
            v.worker_requests_pending, 0,
            "outside cancellation submits no edit"
        );
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
    });
}

#[gpui::test]
fn resize_edge_pan_stops_after_original_audio_handle_leaves_view(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, false, false, false);
}
#[gpui::test]
fn resize_edge_pan_stops_after_original_midi_handle_leaves_view(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, true, false, false, false);
}
#[gpui::test]
fn resize_edge_pan_stops_after_original_shift_handle_leaves_view(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, true, false, false);
}
#[gpui::test]
fn resize_edge_pan_stops_after_original_warp_handle_leaves_view(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, false, true, false);
}
fn check_edge_pan_after_original_handle_leaves_view(
    cx: &mut TestAppContext,
    midi: bool,
    stretch: bool,
    enabled_warp: bool,
    outside_release: bool,
) {
    for left in [false, true] {
        let viewport = Viewport {
            start: if left { 90_000 } else { 174_000 },
            length: 24_000,
        };
        let (handle, editor, track, clip, _files) = fixture(cx, midi, Some(viewport));
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        if enabled_warp {
            let settings = editor.read_with(&visual, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                hex_project::default_audio_warp(
                    p.clip(clip).unwrap(),
                    p.tempo_map(),
                    hex_project::WarpMode::Beats {
                        preserve: hex_project::BeatsPreserve::Transients,
                        tail: hex_project::BeatsTail::Forward,
                        decay: 100,
                    },
                )
                .unwrap()
            });
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings)
                })))
            });
            pump(&editor, &mut visual);
        }
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let down = capture(&mut visual, clip, midi, left);
        // Enabled Warp has a finite retained root: exercise its valid inward
        // crop, not an unsupported extension beyond that root.
        let pan_left = left != enabled_warp;
        let modifiers = Modifiers {
            shift: stretch,
            ..Modifiers::default()
        };
        visual.simulate_modifiers_change(modifiers);
        visual.run_until_parked();
        let edge = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(
                px(if pan_left {
                    lane.left + 3.0
                } else {
                    lane.left + lane.width - 3.0
                }),
                down.y,
            )
        });
        visual.simulate_mouse_move(edge, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let now = Instant::now();
        let selector = Box::leak(
            format!(
                "{}clip-trim-{}-{}",
                if midi { "midi-" } else { "" },
                if left { "left" } else { "right" },
                clip.value()
            )
            .into_boxed_str(),
        );
        for step in 0..24 {
            held_tick(&editor, &mut visual, now + Duration::from_millis(step * 50));
            if visual.debug_bounds(selector).is_none() {
                break;
            }
        }
        if visual.debug_bounds(selector).is_some() {
            editor.read_with(&visual, |v, _| {
                let state = v.state.as_ref().unwrap();
                let active = v.active_trim_drag.as_ref();
                println!("HANDLE_RETAINED_TRACE midi={midi} left={left} stretch={stretch} enabled_warp={enabled_warp} view={:?} audio_active={} midi_active={} pointer={:?} preview={:?} canonical={:?} error={:?}", v.visible_viewport(state.timeline_frames), active.is_some(), v.active_midi_trim_drag.is_some(), active.and_then(|drag| drag.last_pointer_x.get()), active.and_then(|drag| drag.preview.borrow().as_ref().map(|preview| preview.clip.clone())), state.project.clip(clip), v.error);
            });
        }
        assert!(
            visual.debug_bounds(selector).is_none(),
            "actual original handle must have left the rendered viewport"
        );
        let (edge_view, center) = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            (
                v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
                point(px(lane.left + lane.width / 2.0), down.y),
            )
        });
        visual.simulate_mouse_move(center, MouseButton::Left, modifiers);
        visual.run_until_parked();
        held_tick(&editor, &mut visual, now + Duration::from_secs(2));
        let stopped = editor.read_with(&visual, |v, _| {
            let recorded = if midi { v.active_midi_trim_drag.as_ref().unwrap().last_pointer_x.get() } else { v.active_trim_drag.as_ref().unwrap().last_pointer_x.get() };
            let view = v.visible_viewport(v.state.as_ref().unwrap().timeline_frames);
            println!("REAL_RESIZE_INTERIOR midi={midi} left={left} original_handle_absent=true actual_pointer={} recorded={recorded:?} viewport={edge_view:?}->{view:?}", f32::from(center.x));
            view
        });
        assert_eq!(
            stopped, edge_view,
            "held pointer moved to safe interior after source handle unmounted; edge panning must stop"
        );
        assert!(
            editor.read_with(&visual, |v, _| if midi {
                v.active_midi_trim_drag.is_some()
            } else {
                v.active_trim_drag.is_some()
            }),
            "resize remains captured while panning stops"
        );
        // Resume the same edge, cross directly to the opposite edge, then
        // return to the interior again without losing capture or creating history.
        visual.simulate_mouse_move(edge, MouseButton::Left, modifiers);
        visual.run_until_parked();
        held_tick(&editor, &mut visual, now + Duration::from_secs(3));
        let resumed = editor.read_with(&visual, |v, _| {
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames)
        });
        assert!(
            if pan_left {
                resumed.start < stopped.start
            } else {
                resumed.start > stopped.start
            },
            "same edge resumes after interior stop"
        );
        let opposite = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&track];
            point(
                px(if pan_left {
                    lane.left + lane.width - 3.0
                } else {
                    lane.left + 3.0
                }),
                down.y,
            )
        });
        visual.simulate_mouse_move(opposite, MouseButton::Left, modifiers);
        visual.run_until_parked();
        held_tick(&editor, &mut visual, now + Duration::from_secs(4));
        let reversed = editor.read_with(&visual, |v, _| {
            v.visible_viewport(v.state.as_ref().unwrap().timeline_frames)
        });
        assert!(
            if pan_left {
                reversed.start > resumed.start
            } else {
                reversed.start < resumed.start
            },
            "opposite edge reverses immediately"
        );
        visual.simulate_mouse_move(center, MouseButton::Left, modifiers);
        visual.run_until_parked();
        held_tick(&editor, &mut visual, now + Duration::from_secs(5));
        let (expected_audio, expected_midi) = editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.visible_viewport(v.state.as_ref().unwrap().timeline_frames),
                reversed,
                "second interior entry stops ticks again"
            );
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "held preview does not edit canonical state"
            );
            (
                v.active_trim_drag.as_ref().and_then(|drag| {
                    drag.preview
                        .borrow()
                        .as_ref()
                        .map(|preview| preview.clip.clone())
                }),
                v.active_midi_trim_drag
                    .as_ref()
                    .and_then(|drag| *drag.preview.borrow()),
            )
        });
        let release = if outside_release {
            let ruler = visual
                .debug_bounds("arrangement-loop-rail-row")
                .expect("actual ruler receiver");
            let release = point(center.x, ruler.center().y);
            visual.simulate_mouse_move(release, MouseButton::Left, modifiers);
            visual.run_until_parked();
            release
        } else {
            center
        };
        visual.simulate_mouse_up(release, MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.active_trim_drag.is_none() && v.active_midi_trim_drag.is_none());
            let project = &v.state.as_ref().unwrap().project;
            if let Some(expected) = expected_audio.as_ref() {
                assert_eq!(
                    project.clip(clip),
                    Some(expected),
                    "real worker commits last displayed audio extent"
                );
            }
            if let Some(expected) = expected_midi {
                let actual = project.midi_clip(clip).unwrap();
                assert_eq!(
                    (actual.start(), actual.length(), actual.source_start()),
                    (expected.start(), expected.length(), expected.source_start()),
                    "real worker commits last displayed MIDI extent"
                );
            }
            project.snapshot()
        });
        assert_ne!(after, before);
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

#[gpui::test]
fn resize_edge_pan_interior_stop_then_outside_audio_release_commits(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, false, false, true);
}
#[gpui::test]
fn resize_edge_pan_interior_stop_then_outside_midi_release_commits(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, true, false, false, true);
}
#[gpui::test]
fn resize_edge_pan_interior_stop_then_outside_shift_release_commits(cx: &mut TestAppContext) {
    check_edge_pan_after_original_handle_leaves_view(cx, false, true, false, true);
}
