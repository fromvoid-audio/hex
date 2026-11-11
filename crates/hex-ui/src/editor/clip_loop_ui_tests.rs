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
/// The right clip header edge, where option starts an outer repeat (#3796).
fn right_edge(f: &Fixture) -> &'static str {
    Box::leak(
        format!(
            "{}clip-trim-right-{}",
            if f.midi { "midi-" } else { "" },
            f.clip.value()
        )
        .into_boxed_str(),
    )
}
/// `"outer"` option-drags the right header edge; `"cycle"` drags the first
/// repeat boundary.
fn drag(f: &mut Fixture, kind: &str, dx: f32, modifiers: Modifiers) -> gpui::Point<gpui::Pixels> {
    let (selector, modifiers) = if kind == "outer" {
        (
            right_edge(f),
            Modifiers {
                alt: true,
                ..modifiers
            },
        )
    } else {
        (selector(f, kind), modifiers)
    };
    let down = f
        .visual
        .debug_bounds(selector)
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
    // A covering time selection owns the header rail, where option-drag
    // copies the range, so outer repeat is exercised without one.
    for (midi, reversed) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut f = fixture(cx, midi);
        if reversed {
            f.editor.update(&mut f.visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::ReverseClips(vec![f.clip]))));
            });
            pump(&f.editor, &mut f.visual);
        }
        let before = f.editor.read_with(&f.visual, |v, _| {
            v.state.as_ref().unwrap().project.snapshot()
        });
        assert!(
            f.visual.debug_bounds(selector(&f, "outer")).is_none(),
            "no separate repeat corner"
        );
        assert!(
            f.visual
                .debug_bounds("clip-loop-corner-affordance")
                .is_none(),
            "loop action belongs to the edge cursor, not a painted clip icon"
        );
        let trim = f.visual.debug_bounds(right_edge(&f)).unwrap();
        f.visual
            .simulate_mouse_move(trim.center(), None, Modifiers::default());
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::ResizeRightBracket),
            "header right edge trims"
        );
        let option = Modifiers {
            alt: true,
            ..Default::default()
        };
        f.visual.simulate_modifiers_change(option);
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::Loop),
            "option over the header right edge offers repeat"
        );
        f.visual.simulate_modifiers_change(Modifiers::default());
        f.visual.run_until_parked();
        // Option wins over shift: the gesture repeats rather than stretches.
        let modifiers = Modifiers {
            shift: true,
            alt: true,
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
                    "Shift does not turn an option repeat into Warp"
                );
            }
        });
        let overlay = if midi {
            "midi-trim-live-preview"
        } else {
            "trim-live-preview"
        };
        let held = f.visual.debug_bounds(overlay).expect("held painted loop");
        let header = Box::leak(format!("clip-name-{}", f.clip.value()).into_boxed_str());
        let gradients = crate::editor::clip_header_loop::gradient_quads(&mut f.visual, header);
        assert!(!gradients.is_empty());
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
        assert_eq!(
            crate::editor::clip_header_loop::gradient_quads(&mut f.visual, header),
            gradients,
            "released header keeps the prospective gradient until acknowledgement"
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
        assert_eq!(
            crate::editor::clip_header_loop::gradient_quads(&mut f.visual, header),
            gradients,
            "the committed header paints the acknowledged cycle positions"
        );
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
fn clip_loop_compact_row_option_edge_repeats_and_keeps_first_boundary(cx: &mut TestAppContext) {
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
        // A compact row is all header: the whole right edge trims, and
        // option on it repeats.
        assert!(f.visual.debug_bounds(selector(&f, "outer")).is_none());
        let edge = f.visual.debug_bounds(right_edge(&f)).unwrap();
        assert_eq!(edge.size.height, px(23.0));
        f.visual
            .simulate_mouse_move(edge.center(), None, Modifiers::default());
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
            "compact row retains the first-boundary target"
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
        let header = Box::leak(format!("clip-name-{}", f.clip.value()).into_boxed_str());
        let gradient = crate::editor::clip_header_loop::gradient_quads(&mut f.visual, header);
        assert!(!gradient.is_empty());

        f.editor.read_with(&f.visual,|v,_|assert!(v.waveform_geometry_cache.borrow().painted_waveforms().iter().any(|p|
            p.bounds.right()>=held.right()-px(2.0) && p.ordinates.iter().any(|(lo,hi)| hi-lo>1.0)
        ),"ordinary Shift-Warp retains accepted processed root peaks on an existing repeat; warped={warped}"));
        f.visual.simulate_mouse_up(target, MouseButton::Left, shift);
        f.visual.run_until_parked();
        assert_eq!(f.visual.debug_bounds("trim-live-preview"), Some(held));
        assert_eq!(
            crate::editor::clip_header_loop::gradient_quads(&mut f.visual, header),
            gradient
        );

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

#[derive(Debug, PartialEq, Eq)]
enum EdgeMode {
    None,
    Trim,
    Stretch,
    Repeat,
    Fade,
}

/// Press, move past the drag threshold (optionally changing modifiers on the
/// way), and report which gesture the held drag is in.
fn edge_mode(
    f: &mut Fixture,
    at: gpui::Point<gpui::Pixels>,
    start: Modifiers,
    during: Modifiers,
) -> EdgeMode {
    f.visual.simulate_mouse_down(at, MouseButton::Left, start);
    f.visual
        .simulate_mouse_move(point(at.x - px(12.0), at.y), MouseButton::Left, start);
    f.visual.run_until_parked();
    f.visual.simulate_modifiers_change(during);
    f.visual
        .simulate_mouse_move(point(at.x - px(16.0), at.y), MouseButton::Left, during);
    f.visual.run_until_parked();
    let mode = f.editor.read_with(&f.visual, |v, _| {
        let trim = v
            .active_trim_drag
            .as_ref()
            .map(|d| (d.loop_kind, d.repeat.get(), d.stretch_modifier.get()))
            .or_else(|| {
                v.active_midi_trim_drag
                    .as_ref()
                    .map(|d| (d.loop_kind, d.repeat.get(), d.stretch_modifier.get()))
            });
        match trim {
            Some((None, true, _)) => EdgeMode::Repeat,
            Some((None, false, true)) => EdgeMode::Stretch,
            Some((None, false, false)) => EdgeMode::Trim,
            Some(_) => panic!("unexpected loop control drag"),
            None if v.active_fade_drag.is_some() => EdgeMode::Fade,
            None => EdgeMode::None,
        }
    });
    f.visual.simulate_keystrokes("escape");
    f.visual.simulate_mouse_up(at, MouseButton::Left, during);
    f.visual.simulate_modifiers_change(Modifiers::default());
    f.visual.run_until_parked();
    mode
}

#[gpui::test]
fn header_right_edge_modes_follow_the_modifier_at_drag_start_and_the_corner_is_the_fade_handle(
    cx: &mut TestAppContext,
) {
    let none = Modifiers::default();
    let shift = Modifiers {
        shift: true,
        ..none
    };
    let option = Modifiers { alt: true, ..none };
    for midi in [false, true] {
        let mut f = fixture(cx, midi);
        let id = f.clip.value();
        f.visual.run_until_parked();
        let named = |f: &mut Fixture, name: String| {
            f.visual.run_until_parked();
            f.visual.debug_bounds(Box::leak(name.into_boxed_str()))
        };
        let clip = named(
            &mut f,
            format!("{}clip-visual-{id}", if midi { "midi-" } else { "" }),
        )
        .expect("painted clip");
        let trim = f.visual.debug_bounds(right_edge(&f)).unwrap();
        assert!(f.visual.debug_bounds(selector(&f, "outer")).is_none());
        let header = edge_trim_height(f32::from(clip.size.height));
        assert!(
            trim.bottom() <= clip.top() + px(header + 1.0),
            "header rail"
        );
        let edge = trim.center();
        assert_eq!(edge_mode(&mut f, edge, none, none), EdgeMode::Trim);
        assert_eq!(edge_mode(&mut f, edge, shift, shift), EdgeMode::Stretch);
        assert_eq!(edge_mode(&mut f, edge, option, option), EdgeMode::Repeat);
        assert_eq!(
            edge_mode(
                &mut f,
                edge,
                Modifiers {
                    shift: true,
                    ..option
                },
                shift
            ),
            EdgeMode::Repeat,
            "option wins over shift, and releasing it mid-drag keeps repeat"
        );
        assert_eq!(
            edge_mode(&mut f, edge, none, option),
            EdgeMode::Trim,
            "pressing option mid-drag does not switch to repeat"
        );
        let lower_edge = point(edge.x, clip.bottom() - px(12.0));
        assert!(!matches!(
            edge_mode(&mut f, lower_edge, none, none),
            EdgeMode::Trim | EdgeMode::Stretch | EdgeMode::Repeat
        ));
        if !midi {
            // Audio fade handles: the fade-out handle sits in the true corner.
            f.visual
                .simulate_mouse_move(clip.center(), None, Modifiers::default());
            let fade = named(&mut f, format!("fade-out-bottom-{id}")).expect("hover fade");
            assert!((f32::from(fade.center().x - clip.right())).abs() <= 6.0);
            assert!((f32::from(fade.center().y - clip.bottom())).abs() <= 6.0);
            assert_eq!(edge_mode(&mut f, fade.center(), none, none), EdgeMode::Fade);
        }
    }
}

#[gpui::test]
fn option_drag_on_the_header_edge_repeats_with_one_undo(cx: &mut TestAppContext) {
    for midi in [false, true] {
        let mut f = fixture(cx, midi);
        let before = f.editor.read_with(&f.visual, |v, _| {
            v.state.as_ref().unwrap().project.snapshot()
        });
        let target = drag(&mut f, "outer", 70.0, Modifiers::default());
        f.visual.simulate_mouse_up(
            target,
            MouseButton::Left,
            Modifiers {
                alt: true,
                ..Default::default()
            },
        );
        f.visual.run_until_parked();
        let edit = f
            .editor
            .read_with(&f.visual, |v, _| v.last_enqueued_edit.clone());
        assert!(
            matches!(
                edit,
                Some(Command::ResizeAudioClipView {
                    operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { .. },
                    ..
                }) | Some(Command::ResizeMidiClipView {
                    operation: hex_project::MidiViewResizeOperation::RepeatOuterEnd { .. },
                    ..
                })
            ),
            "{edit:?}"
        );
        pump(&f.editor, &mut f.visual);
        f.editor.update(&mut f.visual, |v, _| {
            assert!(v.enqueue(Request::Undo));
        });
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}

#[gpui::test]
fn warped_looping_clip_right_edge_extends_by_repeats_instead_of_cancelling(
    cx: &mut TestAppContext,
) {
    // Reported: warp + looping from the clip panel, then dragging the right
    // edge submitted nothing. A warped trim cannot grow past its retained
    // audio, so the edge of a repeating clip must use the outer repeat.
    for dx in [40.0, 250.0] {
        let mut f = fixture(cx, false);
        let clip = f.clip;
        f.editor.update(&mut f.visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let settings = hex_project::default_audio_warp(
                state.project.clip(clip).unwrap(),
                state.project.tempo_map(),
                hex_project::WarpMode::RePitch,
            )
            .unwrap();
            assert!(v.enqueue(Request::Edit(Command::SetAudioClipWarp {
                clip,
                settings: Some(settings)
            })));
        });
        pump(&f.editor, &mut f.visual);
        f.editor.update(&mut f.visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::SetClipLooping {
                clip,
                looping: true
            })));
        });
        pump(&f.editor, &mut f.visual);
        f.visual.run_until_parked();
        let (end, rate) = f.editor.read_with(&f.visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let clip = state.project.clip(clip).unwrap();
            (clip.start() + clip.duration(), state.project.sample_rate())
        });
        let edge = f
            .visual
            .debug_bounds(Box::leak(
                format!("clip-trim-right-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        f.visual
            .simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
        f.visual.simulate_mouse_move(
            point(edge.x + px(12.0), edge.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        f.visual.run_until_parked();
        let target = point(edge.x + px(dx), edge.y);
        f.visual
            .simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        f.editor.read_with(&f.visual, |v, _| {
            let drag = v.active_trim_drag.as_ref().expect("held edge");
            let preview = drag.preview.borrow();
            let preview = preview
                .as_ref()
                .expect("a growing repeat preview, not none");
            assert!(preview.clip.start() + preview.clip.duration() > end);
        });
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        f.visual.run_until_parked();
        let edit = f
            .editor
            .read_with(&f.visual, |v, _| v.last_enqueued_edit.clone());
        let Some(Command::ResizeAudioClipView {
            clip: edited,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame },
        }) = edit
        else {
            panic!("expected one outer repeat edit, got {edit:?}")
        };
        assert_eq!(edited, clip);
        assert!(
            project_frame > end + u64::from(rate) / 100,
            "{project_frame} vs {end}"
        );
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let canonical = state.project.clip(clip).unwrap();
            assert_eq!(canonical.start() + canonical.duration(), project_frame);
            assert!(loop_handles::audio_repeats(canonical));
        });
    }
}

#[gpui::test]
fn option_repeat_edge_has_priority_over_a_covering_time_selection(cx: &mut TestAppContext) {
    for (midi, boundary) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut f = fixture(cx, midi);
        f.editor.update(&mut f.visual, |view, cx| {
            let state = view.state.as_ref().unwrap();
            let track = state
                .project
                .tracks()
                .iter()
                .find(|track| {
                    if midi {
                        track.midi_clips().iter().any(|clip| clip.id() == f.clip)
                    } else {
                        track.clips().iter().any(|clip| clip.id() == f.clip)
                    }
                })
                .unwrap()
                .id();
            let clip_end = if midi {
                state.midi_preview.get(f.clip).unwrap().frame_end
            } else {
                let clip = state.project.clip(f.clip).unwrap();
                clip.start() + clip.duration()
            };
            view.time_selection = Some(super::super::time_selection::TimeSelection {
                anchor_track: Some(track),
                end_track: Some(track),
                anchor_frame: 0,
                end_frame: if boundary {
                    clip_end
                } else {
                    state.timeline_frames
                },
            });
            cx.notify();
        });
        f.visual.run_until_parked();
        let down = f.visual.debug_bounds(right_edge(&f)).unwrap().center();
        let selection_handle = f
            .visual
            .debug_bounds(Box::leak(
                format!(
                    "time-selection-handle-{}",
                    f.editor.read_with(&f.visual, |view, _| view
                        .time_selection
                        .unwrap()
                        .anchor_track
                        .unwrap()
                        .value())
                )
                .into_boxed_str(),
            ))
            .unwrap();
        let away = point(
            selection_handle.left() + px(5.0),
            selection_handle.center().y,
        );
        f.visual
            .simulate_mouse_move(away, None, Modifiers::default());
        let option = Modifiers {
            alt: true,
            ..Default::default()
        };
        f.visual.simulate_modifiers_change(option);
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::OpenHand),
            "range remains movable away from the clip edge"
        );
        f.visual.simulate_mouse_move(down, None, option);
        f.visual.run_until_parked();
        assert_eq!(
            f.visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::Loop),
            "midi={midi}"
        );
        let target = drag(&mut f, "outer", 70.0, Modifiers::default());
        f.editor.read_with(&f.visual, |view, _| {
            assert!(
                view.time_selection_gesture.is_none() && view.time_range_move_gesture.is_none()
            );
            assert!(if midi {
                view.active_midi_trim_drag.is_some()
            } else {
                view.active_trim_drag.is_some()
            });
        });
        f.visual
            .simulate_mouse_up(target, MouseButton::Left, option);
        f.visual.run_until_parked();
        f.editor
            .read_with(&f.visual, |view, _| assert!(view.pending_trim.is_some()));
        pump(&f.editor, &mut f.visual);
        f.editor.read_with(&f.visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            assert!(if midi {
                project.midi_clip(f.clip).unwrap().looping()
            } else {
                loop_handles::audio_repeats(project.clip(f.clip).unwrap())
            });
        });
    }
}
