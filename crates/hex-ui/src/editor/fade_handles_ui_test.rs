//! Rendered input tests use the production Arrangement and its cached clip layer.
use super::*;
use gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    point, px,
};
use std::sync::Arc;
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
fn authored_fades_show_without_f_and_each_endpoint_and_curve_uses_its_real_handle(
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
        let bounds = visual
            .debug_bounds(selector)
            .expect("authored fade handle always visible");
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
    for cancel in [0, 1, 2] {
        let (project, _, _, clip) = super::super::clip_drag_ui_test::audio_project();
        let (handle, editor) =
            super::super::clip_drag_ui_test::editor_window(cx, project, Arc::default());
        let mut visual = VisualTestContext::from_window(handle, cx);
        key(&mut visual, true, "f");
        let target = visual
            .debug_bounds(Box::leak(
                format!("fade-in-bottom-{}", clip.value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
        let to = point(target.x + px(35.0), target.y);
        visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        if cancel == 0 {
            visual.simulate_keystrokes("escape");
        } else if cancel == 1 {
            visual.update(|window, cx| editor.update(cx, |view, _| view.midi_focus.focus(window)));
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
