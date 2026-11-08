//! Rendered input tests use the production Arrangement and its cached clip layer.
use super::*;
use gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px,
};
use std::sync::Arc;
fn hover_clip(visual: &mut VisualTestContext, clip: ClipId) {
    visual.run_until_parked();
    let selector: &'static str =
        Box::leak(format!("clip-visual-{}", clip.value()).into_boxed_str());
    let center = visual
        .debug_bounds(selector)
        .expect("painted audio clip")
        .center();
    visual.simulate_mouse_move(center, None, Modifiers::default());
    visual.run_until_parked();
}
fn key(visual: &mut VisualTestContext, down: bool, key: &str) {
    if down {
        visual.simulate_event(KeyDownEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            is_held: false,
        });
    } else {
        visual.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).unwrap(),
        });
    }
    visual.run_until_parked();
}
#[gpui::test]
fn held_f_exposes_neutral_handles_and_release_commits_one_fade_without_moving_clip(
    cx: &mut TestAppContext,
) {
    let (project, track, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let original = project.clip(clip).unwrap().clone();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let selector: &'static str =
        Box::leak(format!("fade-in-top-{}", clip.value()).into_boxed_str());
    assert!(visual.debug_bounds(selector).is_none());
    key(&mut visual, true, "f");
    let target = visual
        .debug_bounds(selector)
        .expect("f reveals the actual handle")
        .center();
    editor.update(&mut visual, |view, cx| {
        view.time_selection = Some(super::super::time_selection::TimeSelection {
            anchor_track: track,
            end_track: track,
            anchor_frame: 0,
            end_frame: 96_000,
        });
        cx.notify();
    });
    visual.run_until_parked();
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    let to = point(target.x + px(45.0), target.y);
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.active_fade_drag.as_ref().unwrap().preview.fade_in() > 0);
        assert!(view.active_clip_drag.is_none() && view.active_trim_drag.is_none());
        assert!(
            view.time_selection_gesture.is_none() && view.time_range_move_gesture.is_none(),
            "fade handles take precedence inside an existing time selection"
        );
        assert!(
            view.last_enqueued_edit.is_none(),
            "preview does not enqueue project work"
        );
    });
    key(&mut visual, false, "f");
    assert!(
        visual.debug_bounds(selector).is_some(),
        "active gesture remains visible after f release"
    );
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let command = editor.read_with(&visual, |view, _| {
        assert!(view.active_fade_drag.is_none());
        view.last_enqueued_edit
            .clone()
            .expect("canonical release edit")
    });
    let mut canonical = hex_project::Editor::new(
        editor.read_with(&visual, |v, _| (*v.state.as_ref().unwrap().project).clone()),
        8,
    )
    .unwrap();
    canonical.execute(command).unwrap();
    let changed = canonical.project().clip(clip).unwrap();
    assert_eq!(
        (changed.start(), changed.duration(), changed.source_offset()),
        (
            original.start(),
            original.duration(),
            original.source_offset()
        )
    );
    assert!(changed.fade().fade_in() > 0);
    assert!(canonical.undo());
    assert_eq!(canonical.project().clip(clip).unwrap(), &original);
}
#[gpui::test]
fn authored_fades_reveal_on_hover_and_each_endpoint_and_curve_uses_its_real_handle(
    cx: &mut TestAppContext,
) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let mut canonical = hex_project::Editor::new(project, 16).unwrap();
    let fade = ClipFade::new(96_000, 0, 24_000, 24_000).unwrap();
    canonical
        .execute(Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    for (kind, dx, dy) in [
        (Handle::InBottom, 9.0, 0.0),
        (Handle::OutBottom, -9.0, 0.0),
        (Handle::OutTop, -12.0, 0.0),
        (Handle::InCurve, 0.0, 8.0),
        (Handle::OutCurve, 0.0, -8.0),
    ] {
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            canonical.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, _| {
            view.snap_grid = super::super::musical_time::SnapGrid::Off
        });
        if kind == Handle::InBottom {
            visual.update(|window, cx| editor.update(cx, |view, _| view.midi_focus.focus(window)));
        }
        visual.run_until_parked();
        let selector: &'static str =
            Box::leak(format!("{}-{}", kind.name(), clip.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_none(),
            "resting fades hide controls"
        );
        hover_clip(&mut visual, clip);
        visual.simulate_mouse_move(point(px(5.0), px(5.0)), None, Modifiers::default());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(selector).is_none(),
            "leaving the clip hides controls"
        );
        hover_clip(&mut visual, clip);
        let bounds = visual
            .debug_bounds(selector)
            .expect("hover reveals authored fade handles");
        let header = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clip.value()).into_boxed_str(),
            ))
            .expect("shared clip name strip");
        assert!(
            bounds.origin.y >= header.bottom(),
            "fade controls stay in the waveform body"
        );
        let target = bounds.center();
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.active_fade_drag.is_some(),
                "claiming workspace focus must retain the new gesture"
            )
        });
        let to = point(target.x + px(dx), target.y + px(dy));
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let before = canonical.project().clip(clip).unwrap();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.last_enqueued_edit.is_none(),
                "pointer preview never queues edits"
            );
            if matches!(kind, Handle::InBottom | Handle::OutBottom) {
                let preview = view
                    .fade_trim_preview(clip)
                    .expect("canonical clip preview");
                assert_ne!(preview.duration(), before.duration());
                assert!(preview.start() > before.start() || preview.duration() < before.duration());
            }
        });
        if matches!(kind, Handle::InBottom | Handle::OutBottom) {
            let updated_header = visual
                .debug_bounds(Box::leak(
                    format!("clip-name-{}", clip.value()).into_boxed_str(),
                ))
                .unwrap();
            assert!(
                updated_header.size.width < header.size.width,
                "actual clip header shrinks with the waveform boundary"
            );
        }
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let command = editor.read_with(&visual, |view, _| {
            view.last_enqueued_edit.clone().expect("release edit")
        });
        let mut result = hex_project::Editor::new(canonical.project().clone(), 8).unwrap();
        result.execute(command.clone()).unwrap();
        let changed_clip = result.project().clip(clip).unwrap();
        let changed = changed_clip.retained_fade();
        match kind {
            Handle::InBottom => {
                assert!(matches!(
                    command,
                    Command::TrimClipFadeEdge {
                        edge: TrimEdge::Left,
                        ..
                    }
                ));
                assert_eq!(changed.in_start(), 0);
                assert_eq!(changed_clip.start() + changed.fade_in(), 24_000);
                assert_eq!(changed_clip.start() + changed_clip.duration(), 96_000);
            }
            Handle::OutBottom => {
                assert!(matches!(
                    command,
                    Command::TrimClipFadeEdge {
                        edge: TrimEdge::Right,
                        ..
                    }
                ));
                assert_eq!(changed.out_end_offset(), 0);
                assert_eq!(changed_clip.duration() - changed.fade_out(), 72_000);
                assert_eq!(changed_clip.start(), 0);
            }
            Handle::OutTop => assert!(changed.fade_out() > fade.fade_out()),
            Handle::InCurve => {
                assert!(changed.in_curve() > 0);
                assert_eq!(changed.fade_in(), fade.fade_in());
            }
            Handle::OutCurve => {
                assert!(changed.out_curve() < 0);
                assert_eq!(changed.fade_out(), fade.fade_out());
            }
            _ => panic!("unexpected handle"),
        }
    }
}
#[gpui::test]
fn fade_escape_focus_loss_and_stale_revision_cancel_without_edits(cx: &mut TestAppContext) {
    for kind in [Handle::InBottom, Handle::OutBottom] {
        for cancel in [0, 1, 2] {
            let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
            let (handle, editor) =
                super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
            let mut visual = VisualTestContext::from_window(handle, cx);
            key(&mut visual, true, "f");
            let target = visual
                .debug_bounds(Box::leak(
                    format!("{}-{}", kind.name(), clip.value()).into_boxed_str(),
                ))
                .unwrap()
                .center();
            visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
            let to = point(
                target.x
                    + px(if kind == Handle::InBottom {
                        35.0
                    } else {
                        -35.0
                    }),
                target.y,
            );
            visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            if cancel == 0 {
                visual.simulate_keystrokes("escape");
            } else if cancel == 1 {
                visual.update(|window, cx| {
                    editor.update(cx, |view, _| view.midi_focus.focus(window))
                });
            } else {
                editor.update(&mut visual, |view, _| view.project_generation += 1);
            }
            visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert!(view.active_fade_drag.is_none());
                assert!(
                    view.last_enqueued_edit.is_none(),
                    "cancel {cancel}: {:?}",
                    view.last_enqueued_edit
                );
            });
        }
    }
}

#[gpui::test]
fn f_exposes_editable_handles_for_a_split_whose_retained_fades_are_outside_the_clip(
    cx: &mut TestAppContext,
) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let mut canonical = hex_project::Editor::new(project, 16).unwrap();
    canonical
        .execute(Command::SetClipFades {
            clip,
            fade_in: 1000,
            fade_out: 1000,
        })
        .unwrap();
    // One-shot cuts reanchor fades into each part; a looped clip still keeps
    // the retained split domain, as do legacy saved splits.
    canonical
        .execute(Command::SetClipLooping {
            clip,
            looping: true,
        })
        .unwrap();
    let hex_project::Created::Clip(middle) = canonical
        .execute(Command::SplitClip {
            clip,
            frame: 24_000,
        })
        .unwrap()
    else {
        panic!("split")
    };
    canonical
        .execute(Command::SplitClip {
            clip: middle,
            frame: 72_000,
        })
        .unwrap();
    let source = canonical.project().clip(middle).unwrap();
    assert_eq!(source.fade().gain_at(0), 1.0);
    assert_ne!(source.fade().offset(), 0);
    let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project().clone(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    key(&mut visual, true, "f");
    let target = visual
        .debug_bounds(Box::leak(
            format!("fade-in-top-{}", middle.value()).into_boxed_str(),
        ))
        .expect("f exposes a local edit handle even when the inherited knot is outside this crop")
        .center();
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    key(&mut visual, false, "f");
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("fade-in-top-{}", middle.value()).into_boxed_str()
            ))
            .is_some(),
        "active split controls stay visible after f release before pointer movement"
    );
    let to = point(target.x + px(12.0), target.y);
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let edit = editor.read_with(&visual, |view, _| {
        view.last_enqueued_edit.clone().expect("local fade edit")
    });
    canonical.execute(edit).unwrap();
    let source = canonical.project().clip(middle).unwrap();
    assert_eq!((source.fade().span(), source.fade().offset()), (48_000, 0));
    assert!(source.fade().fade_in() > 1000);
}

#[gpui::test]
fn lower_handles_noop_return_to_origin_and_release_modifier_use_one_trim(cx: &mut TestAppContext) {
    for no_op in [true, false] {
        let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
        let mut canonical = hex_project::Editor::new(project, 8).unwrap();
        canonical
            .execute(Command::SetClipFadeEnvelope {
                clip,
                fade: ClipFade::new(96_000, 0, 40_000, 10_000)
                    .unwrap()
                    .with_shape(4000, 2000, 500, -500)
                    .unwrap(),
            })
            .unwrap();
        let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            canonical.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |view, _| {
            view.snap_grid = super::super::musical_time::SnapGrid::Quarter
        });
        visual.run_until_parked();
        let selector: &'static str =
            Box::leak(format!("fade-in-bottom-{}", clip.value()).into_boxed_str());
        hover_clip(&mut visual, clip);
        let target = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        if no_op {
            // Clicking an old inward endpoint must not absorb its silent offset.
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            assert!(editor.read_with(&visual, |view, _| view.last_enqueued_edit.is_none()));
            visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        }
        let moved = point(target.x + px(13.0), target.y);
        visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        if no_op {
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |view, _| {
                assert!(view.active_fade_drag.is_none());
                assert!(view.last_enqueued_edit.is_none());
            });
        } else {
            let snapped = editor.read_with(&visual, |view, _| {
                view.active_fade_drag.as_ref().unwrap().trim_frame.unwrap()
            });
            visual.simulate_mouse_up(
                moved,
                MouseButton::Left,
                Modifiers {
                    platform: true,
                    ..Modifiers::default()
                },
            );
            let command =
                editor.read_with(&visual, |view, _| view.last_enqueued_edit.clone().unwrap());
            let Command::TrimClipFadeEdge {
                frame,
                edge: TrimEdge::Left,
                ..
            } = command
            else {
                panic!("one trim-and-fade command");
            };
            assert_ne!(
                frame, snapped,
                "release modifier recomputes the edge without another motion"
            );
            canonical.execute(command).unwrap();
            let changed = canonical.project().clip(clip).unwrap();
            assert_eq!(changed.start(), frame);
            assert_eq!(changed.fade().in_start(), 0);
            assert_eq!(changed.start() + changed.fade().fade_in(), 44_000);
            assert!(canonical.undo());
            assert_eq!(
                canonical.project().clip(clip).unwrap().fade().in_start(),
                4000
            );
        }
    }
}

#[gpui::test]
fn held_f_neutral_lower_handle_resizes_without_authoring_a_ramp(cx: &mut TestAppContext) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let mut canonical = hex_project::Editor::new(project.clone(), 8).unwrap();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.snap_grid = super::super::musical_time::SnapGrid::Off
    });
    key(&mut visual, true, "f");
    let target = visual
        .debug_bounds(Box::leak(
            format!("fade-in-bottom-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    let moved = point(target.x + px(15.0), target.y);
    visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = editor.read_with(&visual, |view, _| {
        assert!(view.last_enqueued_edit.is_none());
        view.fade_trim_preview(clip).unwrap().clone()
    });
    assert!(preview.start() > 0);
    assert_eq!(preview.start() + preview.duration(), 96_000);
    assert!(preview.retained_fade().is_neutral());
    key(&mut visual, false, "f");
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
    let command = editor.read_with(&visual, |view, _| view.last_enqueued_edit.clone().unwrap());
    assert!(matches!(command, Command::TrimClipFadeEdge { .. }));
    canonical.execute(command).unwrap();
    assert_eq!(canonical.project().clip(clip).unwrap(), &preview);
}

#[gpui::test]
fn stationary_command_changes_refresh_both_lower_previews_and_release_uses_current_modifiers(
    cx: &mut TestAppContext,
) {
    use super::super::musical_time::SnapGrid;
    for grid in [SnapGrid::Quarter, SnapGrid::Off] {
        for kind in [Handle::InBottom, Handle::OutBottom] {
            for release_snapped in [false, true] {
                let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
                let mut canonical = hex_project::Editor::new(project, 8).unwrap();
                canonical
                    .execute(Command::SetClipFadeEnvelope {
                        clip,
                        fade: ClipFade::new(96_000, 0, 40_000, 40_000).unwrap(),
                    })
                    .unwrap();
                let original = canonical.project().clone();
                let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
                    cx,
                    original.clone(),
                    Arc::default(),
                );
                let mut visual = VisualTestContext::from_window(handle, cx);
                editor.update(&mut visual, |view, _| view.snap_grid = grid);
                visual.run_until_parked();
                let selector: &'static str =
                    Box::leak(format!("{}-{}", kind.name(), clip.value()).into_boxed_str());
                hover_clip(&mut visual, clip);
                let target = visual.debug_bounds(selector).unwrap().center();
                let direction = if kind == Handle::InBottom { 1.0 } else { -1.0 };
                // Clicking without moving remains a no-op even with snap opt-in.
                for modifiers in [
                    Modifiers::default(),
                    Modifiers {
                        platform: true,
                        ..Modifiers::default()
                    },
                ] {
                    visual.simulate_mouse_down(target, MouseButton::Left, modifiers);
                    visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
                    assert!(editor.read_with(&visual, |view, _| view.last_enqueued_edit.is_none()));
                }
                visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
                let dx = editor.read_with(&visual, |view, _| {
                    let drag = view.active_fade_drag.as_ref().unwrap();
                    17_321.0 * drag.lane.width / drag.viewport.length as f32
                });
                let moved = point(target.x + px(dx * direction), target.y);
                visual.simulate_mouse_move(moved, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
                let (free, expected_snapped_frame) = editor.read_with(&visual, |view, _| {
                    let drag = view.active_fade_drag.as_ref().unwrap();
                    let delta = (f64::from(f32::from(moved.x - target.x))
                        / f64::from(drag.lane.width)
                        * drag.viewport.length as f64)
                        .round() as i128;
                    let endpoint = if kind == Handle::InBottom { 0 } else { 96_000 };
                    let raw = (endpoint + delta) as u64;
                    assert_eq!(
                        drag.trim_frame,
                        Some(raw),
                        "{kind:?}/{grid:?}: default is free"
                    );
                    let snap = if grid == SnapGrid::Off {
                        SnapGrid::Adaptive
                    } else {
                        grid
                    };
                    let frame = snap
                        .snap_frame(&original, raw, drag.viewport, drag.lane.width)
                        .unwrap();
                    (view.fade_trim_preview(clip).unwrap().clone(), frame)
                });
                let command = Modifiers {
                    platform: true,
                    ..Modifiers::default()
                };
                visual.simulate_modifiers_change(command);
                visual.run_until_parked();
                let snapped = editor.read_with(&visual, |view, _| {
                    assert!(view.last_enqueued_edit.is_none());
                    assert_eq!(
                        view.active_fade_drag.as_ref().unwrap().trim_frame,
                        Some(expected_snapped_frame)
                    );
                    view.fade_trim_preview(clip).unwrap().clone()
                });
                assert_ne!(
                    free, snapped,
                    "{kind:?}/{grid:?}: stationary Command enables snapping"
                );
                visual.simulate_modifiers_change(Modifiers::default());
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    assert_eq!(view.fade_trim_preview(clip), Some(&free));
                    assert!(view.last_enqueued_edit.is_none());
                    assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
                });
                // Mouse-up can carry a newer modifier than the last modifier event.
                // Exercise release disagreement in both directions, without pointer motion.
                if !release_snapped {
                    visual.simulate_modifiers_change(command);
                    visual.run_until_parked();
                }
                visual.update(|window, cx| {
                    editor.read_with(cx, |view, _| {
                        let drag = view
                            .active_fade_drag
                            .as_ref()
                            .expect("active before release");
                        assert!(
                            view.focus.is_focused(window),
                            "{kind:?}/{grid:?}/{release_snapped}: focus before release"
                        );
                        assert!(
                            view.fade_drag_current(drag),
                            "{kind:?}/{grid:?}/{release_snapped}: current before release"
                        );
                    })
                });
                visual.simulate_mouse_up(
                    moved,
                    MouseButton::Left,
                    if release_snapped {
                        command
                    } else {
                        Modifiers::default()
                    },
                );
                let edit = editor.read_with(&visual, |view, _| {
                    assert!(view.active_fade_drag.is_none());
                    view.last_enqueued_edit.clone().unwrap_or_else(|| {
                        panic!("{kind:?}/{grid:?}/{release_snapped}: release must enqueue")
                    })
                });
                assert!(matches!(edit, Command::TrimClipFadeEdge { .. }));
                canonical.execute(edit).unwrap();
                let expected = if release_snapped { &snapped } else { &free };
                assert_eq!(canonical.project().clip(clip), Some(expected));
                assert!(canonical.undo());
                assert_eq!(canonical.project(), &original);
                assert!(canonical.redo());
                assert_eq!(canonical.project().clip(clip), Some(expected));
            }
        }
    }
}

#[gpui::test]
fn lower_source_limits_are_noops_with_and_without_command(cx: &mut TestAppContext) {
    use super::super::musical_time::SnapGrid;
    for grid in [SnapGrid::Quarter, SnapGrid::Off] {
        for kind in [Handle::InBottom, Handle::OutBottom] {
            for platform in [false, true] {
                // A pending edit from another gesture must not mask source bounds.
                // Keep the pointer inside the window so mouse-up is delivered.
                let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
                let mut canonical = hex_project::Editor::new(project, 8).unwrap();
                canonical
                    .execute(Command::SetClipFadeEnvelope {
                        clip,
                        fade: ClipFade::new(96_000, 0, 40_000, 40_000).unwrap(),
                    })
                    .unwrap();
                let original = canonical.project().clone();
                let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
                    cx,
                    original.clone(),
                    Arc::default(),
                );
                let mut visual = VisualTestContext::from_window(handle, cx);
                editor.update(&mut visual, |view, _| view.snap_grid = grid);
                visual.run_until_parked();
                let selector: &'static str =
                    Box::leak(format!("{}-{}", kind.name(), clip.value()).into_boxed_str());
                hover_clip(&mut visual, clip);
                let target = visual.debug_bounds(selector).unwrap().center();
                let modifiers = Modifiers {
                    platform,
                    ..Modifiers::default()
                };
                visual.simulate_mouse_down(target, MouseButton::Left, modifiers);
                let direction = if kind == Handle::InBottom { -1.0 } else { 1.0 };
                let beyond_source = point(target.x + px(40.0 * direction), target.y);
                visual.simulate_mouse_move(beyond_source, MouseButton::Left, modifiers);
                visual.run_until_parked();
                editor.read_with(&visual, |view, _| {
                    assert!(view.fade_trim_preview(clip).is_none());
                    assert!(view.active_fade_drag.as_ref().unwrap().trim_frame.is_none());
                    assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
                });
                visual.simulate_mouse_up(beyond_source, MouseButton::Left, modifiers);
                editor.read_with(&visual, |view, _| {
                    assert!(view.active_fade_drag.is_none());
                    assert!(view.last_enqueued_edit.is_none());
                    assert_eq!(view.worker_requests_pending, 0);
                });
            }
        }
    }
}

#[gpui::test]
fn two_axis_curve_drags_change_pcm_keep_endpoints_and_bound_vertical_travel(
    cx: &mut TestAppContext,
) {
    for kind in [Handle::InCurve, Handle::OutCurve] {
        for (dx, dy) in [
            (9.0, 0.0),
            (0.0, 8.0),
            (9.0, 8.0),
            (0.0, 500.0),
            (0.0, -500.0),
            (-500.0, -500.0),
            (-500.0, 500.0),
            (500.0, -500.0),
            (500.0, 500.0),
        ] {
            let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
            let mut model = hex_project::Editor::new(project, 8).unwrap();
            let fade = ClipFade::new(96_000, 0, 24_000, 24_000).unwrap();
            model
                .execute(Command::SetClipFadeEnvelope { clip, fade })
                .unwrap();
            let (window, editor) = super::super::clip_drag_ui_test::editor_window(
                cx,
                model.project().clone(),
                Arc::default(),
            );
            let mut visual = VisualTestContext::from_window(window, cx);
            key(&mut visual, true, "f");
            let selector: &'static str =
                Box::leak(format!("{}-{}", kind.name(), clip.value()).into_boxed_str());
            let target = visual.debug_bounds(selector).unwrap().center();
            visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
            let to = point(target.x + px(dx), target.y + px(dy));
            visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let preview = editor.read_with(&visual, |v, _| v.fade_preview(clip).unwrap());
            assert_eq!(
                (
                    preview.fade_in(),
                    preview.fade_out(),
                    preview.in_start(),
                    preview.out_end_offset()
                ),
                (24_000, 24_000, 0, 0)
            );
            let out = kind == Handle::OutCurve;
            let gain = curve_handle_gain(preview, out);
            assert!((0.25..=0.75).contains(&gain));
            let center = visual.debug_bounds(selector).unwrap().center();
            let lane = editor.read_with(&visual, |v, _| v.active_fade_drag.as_ref().unwrap().lane);
            let top = lane.top + super::super::clip_name::CLIP_NAME_HEIGHT;
            let body = lane.height - super::super::clip_name::CLIP_NAME_HEIGHT;
            assert!((f32::from(center.y) - top) >= body * 0.25 - 1.0);
            assert!((f32::from(center.y) - top) <= body * 0.75 + 1.0);
            if dx > 0.0 {
                assert!(center.x > target.x);
            }
            if dx < 0.0 {
                assert!(center.x < target.x);
            }
            if dx.abs() > 100.0 {
                let endpoints = if out {
                    [Handle::OutTop, Handle::OutBottom]
                } else {
                    [Handle::InTop, Handle::InBottom]
                };
                let positions = endpoints.map(|h| {
                    let name: &'static str =
                        Box::leak(format!("{}-{}", h.name(), clip.value()).into_boxed_str());
                    f32::from(visual.debug_bounds(name).unwrap().center().x)
                });
                let edge = if dx < 0.0 {
                    positions[0].min(positions[1])
                } else {
                    positions[0].max(positions[1])
                };
                assert!(
                    (f32::from(center.x) - edge).abs() < 1.0,
                    "diamond reaches the horizontal box edge"
                );
            }
            if dy.abs() > 100.0 {
                let edge = top + body * if dy < 0.0 { 0.25 } else { 0.75 };
                assert!(
                    (f32::from(center.y) - edge).abs() < 1.0,
                    "diamond reaches the vertical box edge"
                );
            }
            if dy == 0.0 {
                assert_eq!((preview.in_curve(), preview.out_curve()), (0, 0));
            }
            if dx == 0.0 {
                assert_eq!((preview.in_position(), preview.out_position()), (500, 500));
            }
            let frame = if out { 84_000 } else { 12_000 };
            assert_ne!(preview.gain_at(frame), fade.gain_at(frame));
            visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
            let command = editor.read_with(&visual, |v, _| v.last_enqueued_edit.clone().unwrap());
            model.execute(command).unwrap();
            assert_eq!(model.project().clip(clip).unwrap().retained_fade(), preview);
            assert!(model.undo());
            assert_eq!(model.project().clip(clip).unwrap().retained_fade(), fade);
            assert!(model.redo());
            assert_eq!(model.project().clip(clip).unwrap().retained_fade(), preview);
        }
    }
}

fn pump(editor: &gpui::Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let ready = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.state.is_some() && !v.edit_pending()
        });
        visual.run_until_parked();
        if ready {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "real worker setup");
        std::thread::yield_now();
    }
}
// Imports one 2 s clip through the real worker and authors 0.5 s fades.
fn real_faded_clip(
    cx: &mut TestAppContext,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::Entity<EditorView>,
    VisualTestContext,
    ClipId,
) {
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let path = files.wav("fade.wav", 48_000, 96_000);
    let (window, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(window, cx);
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Import(path)))
    });
    pump(&editor, &mut visual);
    let clip = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetClipFades {
            clip,
            fade_in: 24_000,
            fade_out: 24_000
        })))
    });
    pump(&editor, &mut visual);
    (files, editor, visual, clip)
}
#[gpui::test]
fn released_push_keeps_both_pushed_handles_painted_until_real_worker_ack(cx: &mut TestAppContext) {
    let (_files, editor, mut visual, clip) = real_faded_clip(cx);
    key(&mut visual, true, "f");
    let in_top: &'static str = Box::leak(format!("fade-in-top-{}", clip.value()).into_boxed_str());
    let out_top: &'static str =
        Box::leak(format!("fade-out-top-{}", clip.value()).into_boxed_str());
    let from = visual.debug_bounds(in_top).unwrap().center();
    let before_out = visual.debug_bounds(out_top).unwrap();
    // Drag the fade in past the fade out's start, then partway back: the fade
    // out gives way and springs back without returning to its original length.
    let past = point(before_out.center().x + px(40.0), from.y);
    let to = point(before_out.center().x + px(20.0), from.y);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(past, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let deeper = editor.read_with(&visual, |v, _| v.fade_preview(clip).unwrap());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let preview = editor.read_with(&visual, |v, _| v.fade_preview(clip).unwrap());
    assert_eq!(deeper.fade_in() + deeper.fade_out(), 96_000);
    assert_eq!(preview.fade_in() + preview.fade_out(), 96_000);
    assert!(deeper.fade_out() < preview.fade_out() && preview.fade_out() < 24_000);
    let held_out = visual.debug_bounds(out_top).unwrap();
    let held_in = visual.debug_bounds(in_top).unwrap();
    assert_eq!(
        held_in, held_out,
        "pushed top handles exercise actual paint overlap"
    );
    super::paint_probe::assert_style(&mut visual, in_top, theme::FadeHandleState::Active);
    assert!(held_out.center().x > before_out.center().x);
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let event = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |v, _| {
                v.receive(event);
            });
        }
        assert!(std::time::Instant::now() < deadline, "real fade terminal");
        std::thread::yield_now();
    };
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(
        event.state.project.clip(clip).unwrap().retained_fade(),
        preview
    );
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(out_top), Some(held_out));
        assert_eq!(visual.debug_bounds(in_top), Some(held_in));
        super::paint_probe::assert_style(&mut visual, in_top, theme::FadeHandleState::Active);
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_fade.is_some());
            assert_eq!(v.fade_preview(clip), Some(preview));
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert!(v.pending_fade.is_none());
        assert_eq!(state.project.clip(clip).unwrap().retained_fade(), preview);
        assert!(state.can_undo);
    });
    assert_eq!(visual.debug_bounds(out_top), Some(held_out));
    assert_eq!(visual.debug_bounds(in_top), Some(held_in));
    super::paint_probe::assert_not_active(&mut visual, in_top);
}

#[gpui::test]
fn released_two_axis_fade_stays_painted_until_real_worker_ack(cx: &mut TestAppContext) {
    use std::time::Instant;
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let path = files.wav("fade.wav", 48_000, 96_000);
    let (window, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(window, cx);
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Import(path)))
    });
    pump(&editor, &mut visual);
    let clip = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].clips()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::SetClipFades {
            clip,
            fade_in: 24_000,
            fade_out: 24_000
        })))
    });
    pump(&editor, &mut visual);
    key(&mut visual, true, "f");
    let selector: &'static str =
        Box::leak(format!("fade-out-curve-{}", clip.value()).into_boxed_str());
    let from = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    let to = point(from.x + px(9.0), from.y + px(8.0));
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds(selector).unwrap();
    let preview = editor.read_with(&visual, |v, _| v.fade_preview(clip).unwrap());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let event = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event()) {
            if event.request_complete {
                break event;
            }
            editor.update(&mut visual, |v, _| {
                v.receive(event);
            });
        }
        assert!(Instant::now() < deadline, "real fade terminal");
        std::thread::yield_now();
    };
    assert!(event.error.is_none(), "{:?}", event.error);
    assert_eq!(
        event.state.project.clip(clip).unwrap().retained_fade(),
        preview
    );
    for _ in 0..3 {
        editor.update(&mut visual, |v, cx| {
            v.cancel_fade_gesture(cx);
            cx.notify();
        });
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector), Some(held));
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_fade.is_some());
            assert_ne!(
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .clip(clip)
                    .unwrap()
                    .retained_fade(),
                preview
            );
            assert_eq!(v.fade_preview(clip), Some(preview));
        });
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(event);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_fade.is_none());
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .clip(clip)
                .unwrap()
                .retained_fade(),
            preview
        );
    });
    hover_clip(&mut visual, clip);
    assert_eq!(visual.debug_bounds(selector), Some(held));

    // A later canonical document replacement cancels an unadmitted drag.
    // Its eventual mouse-up must not apply the old envelope to the new source.
    key(&mut visual, true, "f");
    let from = visual.debug_bounds(selector).unwrap().center();
    let to = point(from.x - px(7.0), from.y + px(6.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let generation = editor.read_with(&visual, |v, _| {
        assert!(v.active_fade_drag.as_ref().is_some_and(|drag| drag.moved));
        v.project_generation
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::NewWithAudioMode {
            mode: hex_session::DocumentAudioMode::Resident,
            reservation: None,
        }));
    });
    pump(&editor, &mut visual);
    let replacement = editor.update(&mut visual, |v, _| {
        assert!(v.project_generation > generation);
        assert!(v.active_fade_drag.is_none() && v.pending_fade.is_none());
        assert!(v.state.as_ref().unwrap().project.tracks().is_empty());
        // Reset only the test edit trace after the expected earlier commit.
        v.last_enqueued_edit = None;
        v.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    for _ in 0..2 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert!(visual.debug_bounds(selector).is_none());
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), replacement);
        assert!(v.last_enqueued_edit.is_none());
        assert!(v.error.is_none() && !v.state.as_ref().unwrap().can_undo);
        assert!(v.active_fade_drag.is_none() && v.pending_fade.is_none());
        assert!(v.pending_requests.is_empty() && v.worker_requests_pending == 0);
    });
}

#[gpui::test]
fn real_refusal_retires_frozen_curve_with_its_canonical_state(cx: &mut TestAppContext) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let mut model = hex_project::Editor::new(project, 8).unwrap();
    model
        .execute(Command::SetClipFades {
            clip,
            fade_in: 24_000,
            fade_out: 24_000,
        })
        .unwrap();
    // The rendered fixture has a clip absent from the real empty Service. That
    // worker must refuse the command; no fabricated completion/state is used.
    let (window, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, model.project().clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    key(&mut visual, true, "f");
    let selector: &'static str =
        Box::leak(format!("fade-in-curve-{}", clip.value()).into_boxed_str());
    let from = visual.debug_bounds(selector).unwrap().center();
    let to = point(from.x + px(9.0), from.y + px(8.0));
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let held = visual.debug_bounds(selector).unwrap();
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let deadline = crate::test_wait::hang_deadline();
    let terminal = loop {
        if let Ok(event) = editor.read_with(&visual, |v, _| v.service.try_event())
            && event.request_complete
        {
            break event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "real refusal terminal"
        );
        std::thread::yield_now();
    };
    assert!(terminal.error.is_some());
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(selector), Some(held));
    }
    editor.update(&mut visual, |v, cx| {
        v.receive(terminal);
        cx.notify();
    });
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert!(v.pending_fade.is_none() && v.fade_preview(clip).is_none())
    });
    assert!(visual.debug_bounds(selector).is_none());
}

#[gpui::test]
fn sample_zoomed_short_curves_follow_subframe_pointer_motion(cx: &mut TestAppContext) {
    for out in [false, true] {
        let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
        let mut model = hex_project::Editor::new(project, 8).unwrap();
        let fade =
            ClipFade::new(96_000, 0, if out { 0 } else { 3 }, if out { 3 } else { 0 }).unwrap();
        model
            .execute(Command::SetClipFadeEnvelope { clip, fade })
            .unwrap();
        let (window, editor) = super::super::clip_drag_ui_test::editor_window(
            cx,
            model.project().clone(),
            Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(window, cx);
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(Viewport {
                start: if out { 95_996 } else { 0 },
                length: 4,
            });
            cx.notify();
        });
        key(&mut visual, true, "f");
        let kind = if out {
            Handle::OutCurve
        } else {
            Handle::InCurve
        };
        let selector: &'static str =
            Box::leak(format!("{}-{}", kind.name(), clip.value()).into_boxed_str());
        let from = visual
            .debug_bounds(selector)
            .expect("sample-zoomed curve handle")
            .center();
        let to = point(from.x + px(20.0), from.y);
        visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let preview = editor.read_with(&visual, |v, _| {
            v.fade_preview(clip)
                .expect("subframe movement edits the curve")
        });
        let moved = visual.debug_bounds(selector).unwrap().center();
        assert!(
            (f32::from(moved.x - from.x) - 20.0).abs() < 1.5,
            "diamond follows the pointer without whole-sample steps"
        );
        assert_eq!(
            (preview.fade_in(), preview.fade_out()),
            (fade.fade_in(), fade.fade_out())
        );
        assert_eq!((preview.in_curve(), preview.out_curve()), (0, 0));
        assert_ne!(
            preview.gain_at(if out { 95_998 } else { 1 }),
            fade.gain_at(if out { 95_998 } else { 1 })
        );
        visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let command = editor.read_with(&visual, |v, _| v.last_enqueued_edit.clone().unwrap());
        model.execute(command).unwrap();
        assert_eq!(model.project().clip(clip).unwrap().retained_fade(), preview);
    }
}

#[gpui::test]
fn hover_reveals_the_held_f_handle_set_on_the_hovered_clip_only(cx: &mut TestAppContext) {
    let (project, _, _, first) = super::super::clip_drag_ui_test::audio_project();
    let mut canonical = hex_project::Editor::new(project, 16).unwrap();
    let hex_project::Created::Clip(second) = canonical
        .execute(Command::SplitClip {
            clip: first,
            frame: 48_000,
        })
        .unwrap()
    else {
        panic!("split")
    };
    // One authored clip (curve handles too) and one fade-free clip.
    canonical
        .execute(Command::SetClipFadeEnvelope {
            clip: first,
            fade: ClipFade::new(48_000, 0, 12_000, 12_000).unwrap(),
        })
        .unwrap();
    let (handle, _) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project().clone(),
        Arc::default(),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    let handles = |visual: &mut VisualTestContext, clip: ClipId| {
        visual.run_until_parked();
        [
            Handle::InTop,
            Handle::InBottom,
            Handle::InCurve,
            Handle::OutTop,
            Handle::OutBottom,
            Handle::OutCurve,
        ]
        .map(|kind| {
            visual.debug_bounds(Box::leak(
                format!("{}-{}", kind.name(), clip.value()).into_boxed_str(),
            ))
        })
    };
    let away = |visual: &mut VisualTestContext| {
        visual.simulate_mouse_move(point(px(5.0), px(5.0)), None, Modifiers::default());
    };
    let none = [None; 6];
    assert_eq!(handles(&mut visual, first), none);
    assert_eq!(handles(&mut visual, second), none);
    for (clip, other, authored) in [(first, second, true), (second, first, false)] {
        away(&mut visual);
        key(&mut visual, true, "f");
        let held = handles(&mut visual, clip);
        assert!(held.iter().filter(|b| b.is_some()).count() == if authored { 6 } else { 4 });
        key(&mut visual, false, "f");
        hover_clip(&mut visual, clip);
        assert_eq!(
            handles(&mut visual, clip),
            held,
            "hover shows the same handles at the same corners as held f"
        );
        assert_eq!(handles(&mut visual, other), none, "only the hovered clip");
    }
    away(&mut visual);
    assert_eq!(handles(&mut visual, first), none);
    assert_eq!(handles(&mut visual, second), none);
}

#[gpui::test]
fn hover_revealed_handle_on_a_fade_free_clip_drags_one_fade_without_moving_clip(
    cx: &mut TestAppContext,
) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let original = project.clip(clip).unwrap().clone();
    assert!(original.retained_fade().is_neutral());
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    hover_clip(&mut visual, clip);
    let target = visual
        .debug_bounds(Box::leak(
            format!("fade-out-top-{}", clip.value()).into_boxed_str(),
        ))
        .expect("hover reveals the fade-free clip's handle")
        .center();
    visual.simulate_mouse_move(target, None, Modifiers::default());
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    let to = point(target.x - px(30.0), target.y);
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert!(view.active_fade_drag.as_ref().unwrap().preview.fade_out() > 0);
        assert!(view.active_clip_drag.is_none() && view.active_trim_drag.is_none());
    });
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    let edit = editor.read_with(&visual, |view, _| {
        assert!(view.active_fade_drag.is_none());
        view.last_enqueued_edit.clone().expect("one fade edit")
    });
    let mut canonical = hex_project::Editor::new(
        editor.read_with(&visual, |v, _| (*v.state.as_ref().unwrap().project).clone()),
        8,
    )
    .unwrap();
    canonical.execute(edit).unwrap();
    let changed = canonical.project().clip(clip).unwrap();
    assert_eq!(
        (changed.start(), changed.duration()),
        (original.start(), original.duration())
    );
    assert!(changed.fade().fade_out() > 0 && changed.fade().fade_in() == 0);
}

#[gpui::test]
fn narrow_clip_hides_hover_and_f_targets_until_zoomed_in(cx: &mut TestAppContext) {
    let (project, track, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let mut model = hex_project::Editor::new(project, 8).unwrap();
    model
        .execute(Command::TrimClip {
            clip,
            start: 0,
            duration: 8_000,
            source_offset: 0,
            source_length: 8_000,
        })
        .unwrap();
    let original = model.project().clone();
    let (window, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, original.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    let lane_width = editor.read_with(&visual, |v, _| v.drag_lanes.get(&track).unwrap().width);
    let selector: &'static str =
        Box::leak(format!("fade-in-top-{}", clip.value()).into_boxed_str());
    for (width, shown) in [(23.0_f32, false), (25.0, true)] {
        editor.update(&mut visual, |v, cx| {
            v.viewport = Some(Viewport {
                start: 0,
                length: (8_000.0 * lane_width / width) as u64,
            });
            cx.notify();
        });
        visual.run_until_parked();
        hover_clip(&mut visual, clip);
        assert_eq!(
            visual.debug_bounds(selector).is_some(),
            shown,
            "hover width {width}"
        );
        key(&mut visual, true, "f");
        assert_eq!(
            visual.debug_bounds(selector).is_some(),
            shown,
            "f width {width}"
        );
        if !shown {
            let clip_bounds = visual
                .debug_bounds(Box::leak(
                    format!("clip-visual-{}", clip.value()).into_boxed_str(),
                ))
                .unwrap();
            let hit = point(
                clip_bounds.left() + px(4.0),
                clip_bounds.top() + px(super::super::clip_name::CLIP_NAME_HEIGHT + 4.0),
            );
            editor.read_with(&visual, |v, _| {
                let state = v.state.as_ref().unwrap();
                let viewport = v.visible_viewport(state.timeline_frames);
                assert!(!v.fade_handle_hit(
                    track,
                    state.project.clip(clip).unwrap(),
                    f32::from(hit.x),
                    f32::from(hit.y),
                    viewport
                ));
            });
            visual.simulate_mouse_down(hit, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(hit, MouseButton::Left, Modifiers::default());
            editor.read_with(&visual, |v, _| assert!(v.active_fade_drag.is_none()));
        }
        if shown {
            let from = visual.debug_bounds(selector).unwrap().center();
            let free = Modifiers {
                platform: true,
                ..Modifiers::default()
            };
            visual.simulate_mouse_down(from, MouseButton::Left, free);
            let to = point(from.x + px(4.0), from.y);
            visual.simulate_mouse_move(to, MouseButton::Left, free);
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| assert!(v.fade_preview(clip).is_some()));
            editor.update(&mut visual, |v, cx| {
                v.viewport = Some(Viewport {
                    start: 0,
                    length: (8_000.0 * lane_width / 23.0) as u64,
                });
                cx.notify();
            });
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(
                    v.active_fade_drag.is_some(),
                    "admitted gesture survives zoom"
                )
            });
            assert!(
                visual.debug_bounds(selector).is_some(),
                "active target remains visible below threshold"
            );
            visual.simulate_mouse_up(to, MouseButton::Left, free);
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(v.pending_fade.is_some(), "release owns its pending preview")
            });
            assert!(
                visual.debug_bounds(selector).is_some(),
                "pending preview remains visible below threshold"
            );
        }
        key(&mut visual, false, "f");
    }
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.as_ref(), &original)
    });
}

#[gpui::test]
fn body_hover_hides_at_the_header_and_f_keeps_its_existing_override(cx: &mut TestAppContext) {
    cx.update(super::super::bind_editor_keys);
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, _) = super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let controls = Box::leak(format!("{}-{}", Handle::InTop.name(), clip.value()).into_boxed_str());
    visual.simulate_mouse_move(header.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(controls).is_none(),
        "header hover cannot reveal fades"
    );
    hover_clip(&mut visual, clip);
    assert!(visual.debug_bounds(controls).is_some());
    visual.simulate_mouse_move(header.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(controls).is_none(),
        "body to header hides fades"
    );
    key(&mut visual, true, "f");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(controls).is_some(),
        "held f works over the header"
    );
    key(&mut visual, false, "f");
    visual.run_until_parked();
    assert!(visual.debug_bounds(controls).is_none());
    visual.simulate_keystrokes("h");
    visual.run_until_parked();
    let compact = visual
        .debug_bounds(Box::leak(
            format!("clip-visual-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    visual.simulate_mouse_move(
        point(compact.center().x, compact.bottom() - px(1.0)),
        None,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds(controls).is_none(),
        "header-only rows have no body hover targets"
    );
}

#[gpui::test]
fn fade_capture_stays_visible_when_dragging_into_the_header(cx: &mut TestAppContext) {
    let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    hover_clip(&mut visual, clip);
    let selector = Box::leak(format!("{}-{}", Handle::InTop.name(), clip.value()).into_boxed_str());
    let knob = visual.debug_bounds(selector).unwrap();
    let header = visual
        .debug_bounds(Box::leak(
            format!("clip-name-{}", clip.value()).into_boxed_str(),
        ))
        .unwrap();
    let end = point(knob.center().x + header.size.width * 0.1, header.center().y);
    visual.simulate_mouse_down(knob.center(), MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| assert!(view.active_fade_drag.is_some()));
    assert!(
        visual.debug_bounds(selector).is_some(),
        "held fade survives header crossing"
    );
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    editor.read_with(cx, |view, _| {
        assert!(view.active_fade_drag.is_none());
        assert!(matches!(
            view.last_enqueued_edit,
            Some(Command::SetClipFadeEnvelope { .. })
        ));
        assert!(view.active_clip_drag.is_none());
    });
}

#[gpui::test]
fn fade_handle_hover_keeps_the_arrow_cursor(cx: &mut TestAppContext) {
    use super::paint_probe::assert_style;
    use crate::theme::FadeHandleState::{Active, Hover, Idle};
    let (_files, _editor, mut visual, clip) = real_faded_clip(cx);
    key(&mut visual, true, "f");
    let other = Box::leak(format!("fade-out-top-{}", clip.value()).into_boxed_str());
    let selector = Box::leak(format!("fade-in-top-{}", clip.value()).into_boxed_str());
    assert_style(&mut visual, other, Idle);
    let target = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_move(target, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::Arrow)
    );
    assert_style(&mut visual, selector, Hover);
    assert_style(&mut visual, other, Idle);
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    assert_style(&mut visual, selector, Active);
    assert_style(&mut visual, other, Idle);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_style(&mut visual, selector, Hover);
    visual.simulate_mouse_move(point(px(0.0), px(0.0)), None, Modifiers::default());
    visual.run_until_parked();
    assert_style(&mut visual, selector, Idle);
    visual.simulate_mouse_move(target, None, Modifiers::default());
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(px(0.0), px(0.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert_style(&mut visual, selector, Active);
    assert_style(&mut visual, other, Idle);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert_style(&mut visual, selector, Idle);
}
