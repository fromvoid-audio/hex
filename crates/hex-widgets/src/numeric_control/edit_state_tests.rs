use super::*;
use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};

#[gpui::test]
fn edit_state_tracks_text_draft_through_commit_and_cancel(cx: &mut TestAppContext) {
    let (handle, control) =
        crate::tests::open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    let state = control.read_with(cx, |control, _| control.edit_state());
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(!state.is_editing());
    let value = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    assert!(state.is_editing());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    visual.simulate_keystrokes("enter");
    assert!(state.is_editing(), "invalid text remains a draft");
    visual.simulate_keystrokes("escape");
    assert!(!state.is_editing());
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.5");
    visual.simulate_keystrokes("enter");
    assert!(!state.is_editing(), "a valid commit retires the draft");
}

#[gpui::test]
fn edit_state_handle_does_not_keep_the_control_alive(cx: &mut TestAppContext) {
    let control = cx.new(|cx| {
        let spec = NumericSpec::new(
            0.0,
            1.0,
            0.5,
            crate::NumericScale::Linear,
            2,
            None::<String>,
        )
        .unwrap();
        NumericControl::new(
            cx,
            "state",
            "state",
            NumericControlKind::HorizontalSlider,
            spec,
            0.5,
            crate::tests::STYLE,
        )
        .unwrap()
    });
    let state = control.read_with(cx, |control, _| control.edit_state());
    control.update(cx, |control, _| control.editing.0.set(true));
    assert!(state.is_editing());
    drop(control);
    cx.update(|_| {});
    assert!(
        !state.is_editing(),
        "the handle does not retain the control"
    );
}
