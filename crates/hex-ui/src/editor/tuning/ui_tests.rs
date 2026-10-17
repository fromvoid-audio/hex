use super::*;
use gpui::{
    AppContext, Focusable, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
};
use std::sync::Arc;

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
