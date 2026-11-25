use super::*;
use gpui::{
    AppContext, Focusable, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
};
use std::sync::Arc;

#[gpui::test]
fn review_first_imported_degree_can_drag_without_changing_reference_or_count(
    cx: &mut TestAppContext,
) {
    let mut snapshot = hex_project::Project::new(48_000).unwrap().snapshot();
    let mut scale = hex_tuning::Scale::equal(12);
    assert!(scale.move_degree(0, 30.0));
    snapshot.tuning = ProjectTuning::new(false, scale, hex_tuning::Audition::default()).unwrap();
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.open = true;
            view.settings.tuning.focus.focus(window);
            view.sync_tuning(cx);
            cx.notify();
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = editor.read_with(&visual, |view, _| {
        view.settings.tuning.pitch_bounds.unwrap()
    });
    let start = gpui::point(
        bounds.left() + bounds.size.width * (30.0 / 1200.0),
        bounds.center().y,
    );
    visual.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.settings.tuning.drag.map(|d| d.degree), Some(0))
    });
    let end = gpui::point(start.x + bounds.size.width * (30.0 / 1200.0), start.y);
    visual.simulate_event(MouseMoveEvent {
        position: end,
        pressed_button: Some(MouseButton::Left),
        modifiers: gpui::Modifiers::default(),
    });
    editor.read_with(&visual, |view, _| {
        let changed = view.settings.tuning.value.scale();
        assert!(changed.cents()[0] > scale.cents()[0]);
        assert_eq!(changed.root_key(), scale.root_key());
        assert_eq!(changed.root_hz(), scale.root_hz());
        assert_eq!(changed.count(), scale.count());
        assert!(!view.settings.tuning.value.enabled());
    });
    visual.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert!(view.settings.tuning.drag.is_none())
    });
}

#[gpui::test]
fn audition_keys_are_owned_by_tuning_focus_and_release_after_modifier_changes(
    cx: &mut TestAppContext,
) {
    cx.update(crate::editor::bind_editor_keys);
    let project = hex_project::Project::new(48000).unwrap();
    let original = project.clone();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.open = true;
            view.settings.tuning.audition_keyboard.enabled = true;
            view.settings.tuning.audition_input = Some(Arc::new(
                hex_engine::tuning_audition::TuningAuditionInput::new(0),
            ));
            view.settings.tuning.focus.focus(window);
            view.sync_tuning(cx);
            cx.notify();
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("tuning-editor").is_some());
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    editor.read_with(cx, |view, _| {
        assert_eq!(
            view.settings.tuning.audition_keyboard.desired_notes()[0].map(|note| note.0),
            Some(60)
        );
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
        assert!(!view.state.as_ref().unwrap().unsaved);
        assert!(
            !view.settings.tuning.value.enabled(),
            "audition does not enable project tuning"
        );
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("shift-a").unwrap(),
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.settings
                .tuning
                .audition_keyboard
                .desired_notes()
                .iter()
                .all(Option::is_none)
        )
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.controls[&Field::Count]
                .entity
                .read(cx)
                .focus_handle(cx)
                .focus(window)
        });
    })
    .unwrap();
    visual.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse("a").unwrap(),
        is_held: false,
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.settings
                .tuning
                .audition_keyboard
                .desired_notes()
                .iter()
                .all(Option::is_none),
            "numeric focus must not play notes"
        )
    });
    let input = editor.read_with(cx, |view, _| {
        Arc::clone(view.settings.tuning.audition_input.as_ref().unwrap())
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| view.close_tuning(window, cx));
    })
    .unwrap();
    assert!(
        input.is_cancelled(),
        "closing cancels even when the worker is busy"
    );
}

#[gpui::test]
fn review_pending_canonical_edit_blocks_new_tuning_gesture_and_drag(cx: &mut TestAppContext) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.open = true;
            view.settings.tuning.focus.focus(window);
            view.sync_tuning(cx);
            cx.notify();
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            let original = view.settings.tuning.value;
            view.pending_requests.push_back(Request::Undo);
            assert!(view.edit_pending());
            view.tuning_change(original.with_enabled(true), NumericEventPhase::Commit);
            assert!(view.settings.tuning.client.snapshot().is_none());
            assert_eq!(view.settings.tuning.value, original);
            let bounds = view.settings.tuning.pitch_bounds.unwrap();
            view.tuning_pointer_down(
                &MouseDownEvent {
                    position: gpui::point(bounds.left(), bounds.center().y),
                    button: MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                },
                window,
                cx,
            );
            assert!(view.settings.tuning.drag.is_none());
        });
    });
}

#[gpui::test]
fn review_followup_dice_changes_the_actual_palette_without_editing_the_project(
    cx: &mut TestAppContext,
) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.open = true;
            view.refresh_tuning_variations(cx);
        })
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let original = editor.read_with(&visual, |view, _| {
        (
            view.settings.tuning.choices.clone(),
            view.settings.tuning.value,
        )
    });
    for _ in 0..2 {
        let before = editor.read_with(&visual, |view, _| view.settings.tuning.choices.clone());
        editor.update(&mut visual, |view, cx| {
            view.regenerate_tuning_variations(cx)
        });
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(
                view.settings.tuning.choices != before,
                "dice must change actual choices, not only the job serial"
            );
            assert_eq!(view.settings.tuning.value, original.1);
            assert_eq!(view.settings.tuning.choices[0], original.1.scale());
            assert!(view.settings.tuning.choices.len() <= 65);
            assert!(
                view.settings
                    .tuning
                    .choices
                    .iter()
                    .all(|scale| scale.valid()
                        && scale.count() == original.1.scale().count()
                        && scale.root_hz() == original.1.scale().root_hz()
                        && scale.root_key() == original.1.scale().root_key())
            );
        });
    }
}

#[gpui::test]
fn review_final_right_click_actions_target_the_node_under_the_pointer(cx: &mut TestAppContext) {
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    cx.update_window(handle, |_, _, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.open = true;
            view.sync_tuning(cx);
            cx.notify();
        })
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = editor.read_with(&visual, |view, _| {
        view.settings.tuning.pitch_bounds.unwrap()
    });
    let point = gpui::point(bounds.left() + bounds.size.width * 0.25, bounds.center().y);
    visual.simulate_mouse_down(point, MouseButton::Right, gpui::Modifiers::default());
    editor.update(&mut visual, |view, cx| {
        assert_eq!(view.settings.tuning.context, Some(3));
        assert_eq!(view.settings.tuning.selected, 3);
        view.tuning_scale_action(4, cx);
        assert_eq!(view.settings.tuning.value.scale().count(), 11);
        assert!(!view.settings.tuning.value.scale().cents()[..11].contains(&300.0));
        assert_eq!(view.settings.tuning.value.scale().cents()[0], 0.0);
    });
}
#[gpui::test]
fn retained_tuning_preview_declines_new_numeric_drag_without_adopting_its_events(
    cx: &mut TestAppContext,
) {
    use hex_session::tuning_gesture::{TuningGestureAck, TuningGesturePhase};
    let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        hex_project::Project::new(48_000).unwrap(),
        Arc::default(),
    );
    editor.update(cx, |view, cx| {
        let original = view.settings.tuning.value;
        let key = view
            .settings
            .tuning
            .client
            .begin(view.project_generation, original)
            .unwrap();
        view.settings.tuning.client.next_request().unwrap();
        assert!(view.settings.tuning.client.acknowledge(&TuningGestureAck {
            key,
            phase: TuningGesturePhase::Begin,
            sequence: 0,
            accepted: true,
            preview_retained: false,
            reason: None,
        }));
        let accepted = view.settings.tuning.client.snapshot().unwrap();
        view.settings
            .tuning
            .client
            .finish(original.with_enabled(true))
            .unwrap();
        let commit = view.settings.tuning.client.next_request().unwrap();
        assert!(view.settings.tuning.client.acknowledge_with_snapshot(
            &TuningGestureAck {
                key,
                phase: TuningGesturePhase::Commit,
                sequence: commit.sequence(),
                accepted: false,
                preview_retained: true,
                reason: Some("publication refused".into()),
            },
            Some(accepted)
        ));
        view.tuning_numeric(
            Field::RootHz,
            NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: 480.0,
                gesture: None,
            },
            cx,
        );
        assert!(view.settings.tuning.numeric_begin_refused);
        assert!(view.settings.tuning.drag.is_none());
        let cancel = view.settings.tuning.client.next_request().unwrap();
        assert_eq!(cancel.phase(), TuningGesturePhase::Cancel);
        assert!(view.settings.tuning.client.acknowledge(&TuningGestureAck {
            key,
            phase: TuningGesturePhase::Cancel,
            sequence: cancel.sequence(),
            accepted: true,
            preview_retained: false,
            reason: None,
        }));
        for phase in [NumericEventPhase::Change, NumericEventPhase::Commit] {
            view.tuning_numeric(
                Field::RootHz,
                NumericControlEvent {
                    phase,
                    value: 480.0,
                    gesture: None,
                },
                cx,
            );
        }
        assert!(
            view.settings.tuning.client.snapshot().is_none(),
            "declined drag cannot acquire a new gesture later"
        );
        assert_eq!(view.settings.tuning.value, original);
        view.tuning_numeric(
            Field::RootHz,
            NumericControlEvent {
                phase: NumericEventPhase::Begin,
                value: 440.0,
                gesture: None,
            },
            cx,
        );
        assert!(!view.settings.tuning.numeric_begin_refused);
        assert!(view.settings.tuning.client.snapshot().is_some());
    });
}

#[gpui::test]
fn pitch_space_hover_highlights_node_and_click_activates_it(cx: &mut TestAppContext) {
    let project = hex_project::Project::new(48_000).unwrap();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.settings.tuning.open = true;
            view.settings.tuning.focus.focus(window);
            view.refresh_tuning_variations(cx);
            view.sync_tuning(cx);
            cx.notify();
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    visual.run_until_parked();

    let (bounds, target_index, target_scale, target_pos) = editor.read_with(&visual, |view, _| {
        let bounds = view.settings.tuning.palette_bounds.expect("palette bounds");
        assert!(view.settings.tuning.choices.len() > 1);
        // Pick the node farthest from all others so its center hits unambiguously.
        let target_index = (1..view.settings.tuning.positions.len())
            .max_by(|&a, &b| {
                let min_dist = |i: usize| {
                    let (ix, iy) =
                        plots::palette_node_center(bounds, view.settings.tuning.positions[i]);
                    view.settings
                        .tuning
                        .positions
                        .iter()
                        .enumerate()
                        .filter(|&(j, _)| j != i)
                        .map(|(_, &p)| {
                            let (jx, jy) = plots::palette_node_center(bounds, p);
                            (ix - jx).hypot(iy - jy)
                        })
                        .fold(f32::INFINITY, f32::min)
                };
                min_dist(a).total_cmp(&min_dist(b))
            })
            .unwrap();
        let (x, y) =
            plots::palette_node_center(bounds, view.settings.tuning.positions[target_index]);
        (
            bounds,
            target_index,
            view.settings.tuning.choices[target_index],
            gpui::point(px(x), px(y)),
        )
    });

    // Hovering empty corner space keeps hover clear.
    let empty = gpui::point(bounds.left() + px(2.0), bounds.top() + px(2.0));
    visual.simulate_event(MouseMoveEvent {
        position: empty,
        pressed_button: None,
        modifiers: gpui::Modifiers::default(),
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.settings.tuning.palette_hover, None);
    });

    // Hovering a variation node lights it up and exposes the pointer cursor.
    visual.simulate_event(MouseMoveEvent {
        position: target_pos,
        pressed_button: None,
        modifiers: gpui::Modifiers::default(),
    });
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.settings.tuning.palette_hover, Some(target_index));
    });
    assert_eq!(
        visual.rendered_cursor_style(),
        Some(gpui::CursorStyle::PointingHand)
    );

    // Clicking the hovered node activates that exact scale.
    visual.simulate_mouse_down(target_pos, MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.settings.tuning.value.scale(), target_scale);
        assert_eq!(view.settings.tuning.palette_hover, None);
    });
}

#[gpui::test]
fn tuning_overlay_occludes_arrangement_clicks_and_double_clicks(cx: &mut TestAppContext) {
    let (project, _source_track, _target_track, clip) =
        crate::editor::clip_drag_ui_test::audio_project();
    let (handle, editor) =
        crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(gpui::size(px(1280.), px(800.)));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();

    let clip_selector = Box::leak(format!("clip-drag-surface-{}", clip.value()).into_boxed_str());
    let clip_point = visual.debug_bounds(clip_selector).unwrap().center();
    let before = editor.read_with(&visual, |view, _| {
        (
            view.selected_track,
            view.selected_clip,
            view.state.as_ref().unwrap().revision,
        )
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, cx| view.open_tuning(window, cx));
    });
    visual.run_until_parked();
    let pending_after_open = editor.read_with(&visual, |view, _| view.worker_requests_pending);

    // Single click, off-target release, and double-click at the hidden clip's
    // coordinates must never hit the arrangement or leave pointer_down behind.
    visual.simulate_mouse_down(clip_point, MouseButton::Left, gpui::Modifiers::default());
    visual.simulate_mouse_up(
        gpui::point(clip_point.x + px(40.0), clip_point.y + px(20.0)),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position: clip_point,
        modifiers: gpui::Modifiers::default(),
        click_count: 2,
        first_mouse: false,
    });
    visual.simulate_mouse_up(clip_point, MouseButton::Left, gpui::Modifiers::default());
    visual.run_until_parked();

    editor.read_with(&visual, |view, _| {
        assert!(view.pointer_down.is_none());
        assert!(view.clip_paint_gesture.is_none());
        assert_eq!(view.selected_track, before.0);
        assert_eq!(view.selected_clip, before.1);
        assert_eq!(view.state.as_ref().unwrap().revision, before.2);
        assert_eq!(view.worker_requests_pending, pending_after_open);
        assert!(view.test_probes.last_enqueued_edit.is_none());
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            // Drain the read-only tuning library list before checking arrangement availability.
            view.worker_requests_pending = 0;
            view.close_tuning(window, cx);
        });
    });
    visual.run_until_parked();
    editor.update(&mut visual, |view, cx| {
        assert!(view.arrangement_view_change_available(cx));
    });
}
