//! Real pointer capture and Service terminals, sharing the ordinary trim fixture.
use super::resize_release_tests::{fixture as audio_fixture, pump};
use super::*;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext, point};

struct Fixture {
    _files: crate::editor::external_audio_drop_ui_test::BatchFiles,
    visual: VisualTestContext,
    editor: Entity<EditorView>,
    clip: hex_project::ClipId,
    midi: bool,
}
fn fixture(cx: &mut TestAppContext, midi: bool) -> Fixture {
    let (files, handle, editor, mut clip) = audio_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    if midi {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddTrack {
                name: "loop source".into()
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
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(4).unwrap()
            })));
        });
        pump(&editor, &mut visual);
        clip = editor.read_with(&visual, |v, _| {
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
            assert!(v.enqueue(Request::Edit(Command::SetMidiClipSource {
                clip,
                source_length: BeatTime::from_quarters(8).unwrap(),
                source_start: BeatTime::ONE,
                loop_start: BeatTime::ONE,
                loop_end: BeatTime::from_quarters(5).unwrap(),
                looping: false,
                length: BeatTime::from_quarters(4).unwrap(),
            })));
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::from_quarters(2).unwrap(),
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })));
        });
    } else {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::TrimClipEdge {
                clip,
                edge: TrimEdge::Left,
                frame: 108_000
            })));
        });
    }
    pump(&editor, &mut visual);
    visual.run_until_parked();
    Fixture {
        _files: files,
        visual,
        editor,
        clip,
        midi,
    }
}
fn selector(f: &Fixture, kind: &str) -> &'static str {
    Box::leak(
        format!(
            "{}clip-loop-{kind}-{}",
            if f.midi { "midi-" } else { "" },
            f.clip.value()
        )
        .into_boxed_str(),
    )
}
fn drag(f: &mut Fixture, kind: &str, dx: f32, modifiers: Modifiers) -> gpui::Point<gpui::Pixels> {
    let down = f
        .visual
        .debug_bounds(selector(f, kind))
        .expect("loop handle")
        .center();
    f.visual
        .simulate_mouse_down(down, MouseButton::Left, modifiers);
    f.visual.simulate_mouse_move(
        point(down.x + px(if dx < 0.0 { -12.0 } else { 12.0 }), down.y),
        MouseButton::Left,
        modifiers,
    );
    f.visual.run_until_parked();
    let target = point(down.x + px(dx), down.y);
    f.visual
        .simulate_mouse_move(target, MouseButton::Left, modifiers);
    f.visual.run_until_parked();
    target
}

#[gpui::test]
fn clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge(
    cx: &mut TestAppContext,
) {
    for (midi, reversed, selected_range) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
    ] {
        let mut f = fixture(cx, midi);
        if reversed {
            f.editor.update(&mut f.visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::ReverseClips(vec![f.clip]))));
            });
            pump(&f.editor, &mut f.visual);
        }
        if selected_range {
            f.editor.update(&mut f.visual, |v, cx| {
                let state = v.state.as_ref().unwrap();
                let track = state
                    .project
                    .tracks()
                    .iter()
                    .find(|track| {
                        track.clips().iter().any(|clip| clip.id() == f.clip)
                            || track.midi_clips().iter().any(|clip| clip.id() == f.clip)
                    })
                    .unwrap()
                    .id();
                v.time_selection = Some(crate::editor::time_selection::TimeSelection {
                    anchor_track: track,
                    end_track: track,
                    anchor_frame: 0,
                    end_frame: state.timeline_frames,
                });
                v.time_selection_project_scope = false;
                v.selected_clip = None;
                cx.notify();
            });
            f.visual.run_until_parked();
        }
        let before = f.editor.read_with(&f.visual, |v, _| {
            v.state.as_ref().unwrap().project.snapshot()
        });
        let handle = f.visual.debug_bounds(selector(&f, "outer")).unwrap();
        assert!(
            f.visual
                .debug_bounds("clip-loop-corner-affordance")
                .is_none(),
            "loop action belongs to the edge cursor, not a painted clip icon"
        );
        let trim_selector = Box::leak(
            format!(
                "{}clip-trim-right-{}",
                if midi { "midi-" } else { "" },
                f.clip.value()
            )
            .into_boxed_str(),
        );
        // Edge trim lives in the header rail; the loop corner keeps the
        // bottom of the same edge, with ordinary body between them.
        let trim = f.visual.debug_bounds(trim_selector).unwrap();
        assert!((handle.right() - trim.right()).abs() <= px(1.0));
        assert!(trim.bottom() < handle.top());
        f.visual
            .simulate_mouse_move(handle.center(), None, Modifiers::default());
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::Loop)
        );
        f.visual
            .simulate_mouse_move(trim.center(), None, Modifiers::default());
        f.visual.run_until_parked();
        // A covering time selection owns the header rail (it moves the range).
        if !selected_range {
            assert_eq!(
                f.visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::ResizeRightBracket),
                "header right edge trims"
            );
        }
        f.visual.simulate_mouse_move(
            point(handle.center().x, handle.top() - px(3.0)),
            None,
            Modifiers::default(),
        );
        f.visual.run_until_parked();
        assert_ne!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ResizeRightBracket),
            "the lower right edge above the loop corner no longer trims"
        );
        let modifiers = Modifiers {
            shift: !selected_range,
            ..Default::default()
        };
        let target = drag(&mut f, "outer", 70.0, modifiers);
        f.editor.read_with(&f.visual, |v, _| {
            assert!(v.time_selection_gesture.is_none());
            if midi {
                assert!(matches!(
                    *v.active_midi_trim_drag
                        .as_ref()
                        .expect("held loop")
                        .preview
                        .borrow(),
                    Some(MidiEdgePreview::Loop(_))
                ));
            } else {
                let p = v
                    .active_trim_drag
                    .as_ref()
                    .expect("held loop")
                    .preview
                    .borrow();
                assert!(loop_handles::audio_repeats(&p.as_ref().unwrap().clip));
                assert!(
                    !p.as_ref()
                        .unwrap()
                        .clip
                        .audio_edit()
                        .unwrap()
                        .domain
                        .warp_enabled,
                    "Shift does not turn a loop handle into Warp"
                );
            }
        });
        let overlay = if midi {
            "midi-trim-live-preview"
        } else {
            "trim-live-preview"
        };
        let held = f.visual.debug_bounds(overlay).expect("held painted loop");
        let marker = f
            .visual
            .debug_bounds("clip-repeat-marker-0")
            .expect("first repeat marker");
        assert!(marker.left() > held.left() && marker.right() < held.right());
        assert!(f.visual.rendered_border(overlay).is_some());
        let body = f
            .visual
            .debug_bounds("clip-waveform-body")
            .expect("loop body");
        assert!(
            body.bottom() <= held.bottom() - px(loop_handles::BOTTOM_RADIUS),
            "child fills stay clear of rounded lower corners"
        );
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, modifiers);
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.debug_bounds(overlay),
            Some(held),
            "released preview survives completed paints before ACK"
        );
        assert_eq!(
            f.visual.debug_bounds("clip-repeat-marker-0"),
            Some(marker),
            "marker stays at prospective boundary before ACK"
        );
        f.editor.read_with(&f.visual, |v, _| {
            assert!(v.pending_trim.is_some());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            assert!(if midi {
                matches!(
                    v.last_enqueued_edit,
                    Some(Command::ResizeMidiClipView { .. })
                )
            } else {
                matches!(
                    v.last_enqueued_edit,
                    Some(Command::ResizeAudioClipView { .. })
                )
            });
        });
        pump(&f.editor, &mut f.visual);
        let after = f.editor.read_with(&f.visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(v.pending_trim.is_none());
            let project = &v.state.as_ref().unwrap().project;
            assert!(if midi {
                project.midi_clip(f.clip).unwrap().looping()
            } else {
                loop_handles::audio_repeats(project.clip(f.clip).unwrap())
            });
            let directed = if midi {
                project
                    .midi_clip(f.clip)
                    .unwrap()
                    .content_view()
                    .is_some_and(|v| v.reversed)
            } else {
                project
                    .clip(f.clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .content_view
                    .is_some_and(|v| v.window.reversed)
            };
            assert_eq!(directed, reversed, "loop resizing retains direction");
            project.snapshot()
        });
        if reversed {
            assert!(
                f.visual.debug_bounds(selector(&f, "cycle")).is_none(),
                "a directed seam is not a forward authoring handle"
            );
        }
        f.visual.simulate_keystrokes("cmd-z");
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        f.visual.simulate_keystrokes("cmd-shift-z");
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}

#[gpui::test]
fn clip_loop_first_boundary_keeps_outer_extent_and_escape_does_not_commit(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let mut f = fixture(cx, midi);
        let target = drag(&mut f, "outer", 70.0, Modifiers::default());
        f.visual.update(|_, cx| {
            assert_eq!(cx.active_drag_cursor_style(), Some(gpui::CursorStyle::Loop))
        });
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&f.editor, &mut f.visual);
        let before = f.editor.read_with(&f.visual, |v, _| {
            v.state.as_ref().unwrap().project.snapshot()
        });
        let cycle = f.visual.debug_bounds(selector(&f, "cycle")).unwrap();
        f.visual
            .simulate_mouse_move(cycle.center(), None, Modifiers::default());
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ResizeLeftRight)
        );
        let target = drag(&mut f, "cycle", -35.0, Modifiers::default());
        f.visual.update(|_, cx| {
            assert_eq!(
                cx.active_drag_cursor_style(),
                Some(gpui::CursorStyle::ResizeLeftRight)
            )
        });
        // Active cursor follows the admitted handle outside its original hit zone.
        f.visual.simulate_mouse_move(
            point(px(2.), px(2.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ResizeLeftRight)
        );
        f.visual
            .simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        let overlay = if midi {
            "midi-trim-live-preview"
        } else {
            "trim-live-preview"
        };
        let held = f.visual.debug_bounds(overlay).unwrap();
        f.visual.simulate_keystrokes("escape");
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        f.visual
            .update(|_, cx| assert_eq!(cx.active_drag_cursor_style(), None));
        f.editor.read_with(&f.visual, |v, _| {
            assert!(
                v.active_trim_drag.is_none()
                    && v.active_midi_trim_drag.is_none()
                    && v.pending_trim.is_none()
            );
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        let target = drag(&mut f, "cycle", -35.0, Modifiers::default());
        assert_eq!(
            f.visual.debug_bounds(overlay),
            Some(held),
            "changing a cycle does not resize outer occupancy"
        );
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&f.editor, &mut f.visual);
        f.visual
            .update(|_, cx| assert_eq!(cx.active_drag_cursor_style(), None));
        f.editor.read_with(&f.visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert_ne!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
        f.visual.simulate_keystrokes("cmd-z");
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn clip_loop_no_motion_stale_and_refused_capture_leave_no_edit(cx: &mut TestAppContext) {
    for midi in [false, true] {
        for case in 0..3 {
            let mut f = fixture(cx, midi);
            let before = f.editor.read_with(&f.visual, |v, _| {
                v.state.as_ref().unwrap().project.snapshot()
            });
            f.editor
                .update(&mut f.visual, |v, _| v.last_enqueued_edit = None);
            let target = drag(
                &mut f,
                "outer",
                if case == 0 { 0.0 } else { 70.0 },
                Modifiers::default(),
            );
            f.editor.update(&mut f.visual, |v, _| match case {
                1 => v.project_generation += 1,
                2 => v.close_prompt_open = true,
                _ => {}
            });
            f.visual
                .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            f.visual.run_until_parked();
            f.editor.read_with(&f.visual, |v, _| {
                assert!(
                    v.pending_trim.is_none()
                        && v.active_trim_drag.is_none()
                        && v.active_midi_trim_drag.is_none(),
                    "midi={midi} case={case}"
                );
                assert!(v.last_enqueued_edit.is_none());
                assert_eq!(v.worker_requests_pending, 0);
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}

#[gpui::test]
fn clip_loop_compact_track_corner_keeps_ordinary_edge_above(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let mut f = fixture(cx, midi);
        let track = f.editor.read_with(&f.visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .iter()
                .find(|t| {
                    t.clips().iter().any(|c| c.id() == f.clip)
                        || t.midi_clips().iter().any(|c| c.id() == f.clip)
                })
                .unwrap()
                .id()
        });
        let resize = Box::leak(format!("track-height-resize-{}", track.value()).into_boxed_str());
        let down = f.visual.debug_bounds(resize).unwrap().center();
        let target = point(down.x, down.y - px(100.0));
        f.visual
            .simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        f.visual
            .simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        f.editor
            .read_with(&f.visual, |v, _| assert!(v.track_resize_active()));
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        f.editor.read_with(&f.visual, |v, _| {
            assert_eq!(
                v.track_height(track),
                super::super::track_layout::HEADER_ONLY_HEIGHT
            )
        });
        let corner = f
            .visual
            .debug_bounds(selector(&f, "outer"))
            .expect("compact loop handle");
        let edge = Box::leak(
            format!(
                "{}clip-trim-right-{}",
                if midi { "midi-" } else { "" },
                f.clip.value()
            )
            .into_boxed_str(),
        );
        let edge = f.visual.debug_bounds(edge).unwrap();
        assert!((corner.right() - edge.right()).abs() <= px(1.0));
        assert!(corner.top() > edge.top() + px(3.0));
        f.visual.simulate_mouse_move(
            point(corner.center().x, corner.top() - px(2.0)),
            None,
            Modifiers::default(),
        );
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ResizeRightBracket)
        );
        let target = drag(&mut f, "outer", 40.0, Modifiers::default());
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&f.editor, &mut f.visual);
        assert!(
            f.visual.debug_bounds(selector(&f, "cycle")).is_some(),
            "compact row retains the first-boundary target beside the corner"
        );
        f.editor.read_with(&f.visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert!(if midi {
                p.midi_clip(f.clip).unwrap().looping()
            } else {
                loop_handles::audio_repeats(p.clip(f.clip).unwrap())
            });
        });
    }
}

#[gpui::test]
fn clip_loop_audio_preview_keeps_nonzero_waveform_across_the_extended_body(
    cx: &mut TestAppContext,
) {
    for reversed in [false, true] {
        let mut f = fixture(cx, false);
        if reversed {
            f.editor.update(&mut f.visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::ReverseClips(vec![f.clip]))));
            });
            pump(&f.editor, &mut f.visual);
        }
        f.editor.update(&mut f.visual, |v, _| {
            v.waveform_geometry_cache.borrow_mut().clear()
        });
        let target = drag(&mut f, "outer", 70.0, Modifiers::default());
        let held = f.visual.debug_bounds("trim-live-preview").unwrap();
        for released in [false, true] {
            if released {
                f.visual
                    .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
                f.visual.run_until_parked();
                assert_eq!(f.visual.debug_bounds("trim-live-preview"), Some(held));
            }
            f.editor.read_with(&f.visual, |v, _| {
                assert!(
                    v.waveform_geometry_cache
                        .borrow()
                        .painted_waveforms()
                        .iter()
                        .any(|p| p.bounds.right() >= held.right() - px(2.0)
                            && p.ordinates.iter().any(|(lo, hi)| hi - lo > 1.0)),
                    "held loop must paint accepted nonzero peaks through its new outer edge"
                );
            });
        }
    }
}

#[gpui::test]
fn clip_loop_existing_repeat_shift_warp_keeps_processed_waveform_and_real_history(
    cx: &mut TestAppContext,
) {
    for warped in [false, true] {
        let mut f = fixture(cx, false);
        if warped {
            let settings = f.editor.read_with(&f.visual, |v, _| {
                let p = &v.state.as_ref().unwrap().project;
                hex_project::default_audio_warp(
                    p.clip(f.clip).unwrap(),
                    p.tempo_map(),
                    hex_project::WarpMode::RePitch,
                )
                .unwrap()
            });
            f.editor.update(&mut f.visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                    clip: f.clip,
                    settings: Some(settings)
                })))
            });
            pump(&f.editor, &mut f.visual);
        }
        let target = drag(&mut f, "outer", 70.0, Modifiers::default());
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&f.editor, &mut f.visual);
        let before = f.editor.read_with(&f.visual, |v, _| {
            v.state.as_ref().unwrap().project.snapshot()
        });
        let edge = Box::leak(format!("clip-trim-right-{}", f.clip.value()).into_boxed_str());
        let down = f.visual.debug_bounds(edge).unwrap().center();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        f.editor.update(&mut f.visual, |v, _| {
            v.waveform_geometry_cache.borrow_mut().clear()
        });
        f.visual.simulate_mouse_down(down, MouseButton::Left, shift);
        f.visual
            .simulate_mouse_move(point(down.x + px(12.0), down.y), MouseButton::Left, shift);
        f.visual.run_until_parked();
        let target = point(down.x + px(40.0), down.y);
        f.visual
            .simulate_mouse_move(target, MouseButton::Left, shift);
        f.visual.run_until_parked();
        let held = f.visual.debug_bounds("trim-live-preview").unwrap();
        f.editor.read_with(&f.visual,|v,_|assert!(v.waveform_geometry_cache.borrow().painted_waveforms().iter().any(|p|
            p.bounds.right()>=held.right()-px(2.0) && p.ordinates.iter().any(|(lo,hi)| hi-lo>1.0)
        ),"ordinary Shift-Warp retains accepted processed root peaks on an existing repeat; warped={warped}"));
        f.visual.simulate_mouse_up(target, MouseButton::Left, shift);
        f.visual.run_until_parked();
        assert_eq!(f.visual.debug_bounds("trim-live-preview"), Some(held));
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error)
        });
        f.visual.simulate_keystrokes("cmd-z");
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
    }
}

#[gpui::test]
fn repeat_boundary_cursor_retires_with_focus_and_window_loss(cx: &mut TestAppContext) {
    for midi in [false, true] {
        for window_loss in [false, true] {
            let mut f = fixture(cx, midi);
            f.visual.update(|window, _| window.activate_window());
            f.visual.run_until_parked();
            let target = drag(&mut f, "outer", 70., Modifiers::default());
            f.visual
                .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            pump(&f.editor, &mut f.visual);
            let before = f.editor.read_with(&f.visual, |v, _| {
                v.state.as_ref().unwrap().project.snapshot()
            });
            let target = drag(&mut f, "cycle", -35., Modifiers::default());
            f.visual.update(|_, cx| {
                assert_eq!(
                    cx.active_drag_cursor_style(),
                    Some(gpui::CursorStyle::ResizeLeftRight)
                )
            });
            if window_loss {
                f.visual.deactivate_window();
            } else {
                f.visual
                    .update(|window, cx| f.editor.update(cx, |v, cx| v.search_browser(window, cx)));
            }
            f.visual.run_until_parked();
            f.visual.update(|_, cx| {
                assert_eq!(
                    cx.active_drag_cursor_style(),
                    None,
                    "midi={midi} window_loss={window_loss}"
                );
            });
            f.visual
                .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            f.visual
                .simulate_mouse_move(point(px(2.), px(2.)), None, Modifiers::default());
            f.visual.run_until_parked();
            assert_ne!(
                f.visual.rendered_cursor_style(),
                Some(gpui::CursorStyle::ResizeLeftRight)
            );
            f.editor.read_with(&f.visual, |v, _| {
                assert!(
                    v.active_trim_drag.is_none()
                        && v.active_midi_trim_drag.is_none()
                        && v.pending_trim.is_none()
                );
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        }
    }
}

#[gpui::test]
fn edge_trim_starts_only_in_the_header_and_loop_corner_and_fade_handle_stay_apart(
    cx: &mut TestAppContext,
) {
    let mut f = fixture(cx, false);
    let id = f.clip.value();
    let bounds = |f: &mut Fixture, name: String| {
        f.visual.run_until_parked();
        f.visual
            .debug_bounds(Box::leak(name.into_boxed_str()))
            .unwrap_or_else(|| panic!("painted {id}"))
    };
    let clip = bounds(&mut f, format!("clip-visual-{id}"));
    let trim = bounds(&mut f, format!("clip-trim-right-{id}"));
    let outer = f.visual.debug_bounds(selector(&f, "outer")).unwrap();
    let header = edge_trim_height(f32::from(clip.size.height));
    assert!(
        trim.bottom() <= clip.top() + px(header + 1.0),
        "trim strip is the header rail"
    );
    // Hover reveals the f handle set; the fade-out corner sits inside the loop corner.
    f.visual
        .simulate_mouse_move(clip.center(), None, Modifiers::default());
    let fade = bounds(&mut f, format!("fade-out-bottom-{id}"));
    assert!(!fade.intersects(&outer), "fade {fade:?} loop {outer:?}");
    let cursor_at = |f: &mut Fixture, at| {
        f.visual.simulate_mouse_move(at, None, Modifiers::default());
        f.visual.run_until_parked();
        f.visual.rendered_cursor_style()
    };
    assert_eq!(
        cursor_at(&mut f, outer.center()),
        Some(gpui::CursorStyle::Loop)
    );
    assert_eq!(
        cursor_at(&mut f, trim.center()),
        Some(gpui::CursorStyle::ResizeRightBracket)
    );
    let lower_edge = point(trim.center().x, outer.top() - px(4.0));
    assert_ne!(
        cursor_at(&mut f, lower_edge),
        Some(gpui::CursorStyle::ResizeRightBracket)
    );
    let press = |f: &mut Fixture, at: gpui::Point<gpui::Pixels>| {
        f.visual
            .simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        f.visual.simulate_mouse_move(
            point(at.x - px(12.0), at.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        f.visual.run_until_parked();
        let state = f.editor.read_with(&f.visual, |v, _| {
            (
                v.active_trim_drag.as_ref().map(|d| d.loop_kind),
                v.active_fade_drag.is_some(),
            )
        });
        f.visual.simulate_keystrokes("escape");
        f.visual
            .simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        state
    };
    assert_eq!(
        press(&mut f, trim.center()),
        (Some(None), false),
        "header edge trims"
    );
    assert_eq!(
        press(&mut f, lower_edge).0,
        None,
        "lower edge is body, not trim"
    );
    assert_eq!(
        press(&mut f, outer.center()),
        (Some(Some(LoopHandleKind::Outer)), false),
        "loop corner keeps its own hit box"
    );
    f.visual
        .simulate_mouse_move(clip.center(), None, Modifiers::default());
    let fade = bounds(&mut f, format!("fade-out-bottom-{id}"));
    assert_eq!(
        press(&mut f, fade.center()),
        (None, true),
        "fade handle drags"
    );
}
