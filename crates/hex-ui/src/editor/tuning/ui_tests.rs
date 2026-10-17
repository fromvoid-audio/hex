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
            view.tuning.open = true;
            view.tuning.focus.focus(window);
            view.sync_tuning(cx);
            cx.notify();
        });
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let bounds = editor.read_with(&visual, |view, _| view.tuning.pitch_bounds.unwrap());
    let start = gpui::point(
        bounds.left() + bounds.size.width * (30.0 / 1200.0),
        bounds.center().y,
    );
    visual.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.tuning.drag.map(|d| d.degree), Some(0))
    });
    let end = gpui::point(start.x + bounds.size.width * (30.0 / 1200.0), start.y);
    visual.simulate_event(MouseMoveEvent {
        position: end,
        pressed_button: Some(MouseButton::Left),
        modifiers: gpui::Modifiers::default(),
    });
    editor.read_with(&visual, |view, _| {
        let changed = view.tuning.value.scale();
        assert!(changed.cents()[0] > scale.cents()[0]);
        assert_eq!(changed.root_key(), scale.root_key());
        assert_eq!(changed.root_hz(), scale.root_hz());
        assert_eq!(changed.count(), scale.count());
        assert!(!view.tuning.value.enabled());
    });
    visual.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::default());
    editor.read_with(&visual, |view, _| assert!(view.tuning.drag.is_none()));
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
            view.tuning.open = true;
            view.tuning.audition_keyboard.enabled = true;
            view.tuning.audition_input = Some(Arc::new(
                hex_engine::tuning_audition::TuningAuditionInput::new(0),
            ));
            view.tuning.focus.focus(window);
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
            view.tuning.audition_keyboard.desired_notes()[0].map(|note| note.0),
            Some(60)
        );
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
        assert!(!view.state.as_ref().unwrap().unsaved);
        assert!(
            !view.tuning.value.enabled(),
            "audition does not enable project tuning"
        );
    });
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("shift-a").unwrap(),
    });
    editor.read_with(cx, |view, _| {
        assert!(
            view.tuning
                .audition_keyboard
                .desired_notes()
                .iter()
                .all(Option::is_none)
        )
    });
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |view, cx| {
            view.tuning.controls[&Field::Count]
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
            view.tuning
                .audition_keyboard
                .desired_notes()
                .iter()
                .all(Option::is_none),
            "numeric focus must not play notes"
        )
    });
    let input = editor.read_with(cx, |view, _| {
        Arc::clone(view.tuning.audition_input.as_ref().unwrap())
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
