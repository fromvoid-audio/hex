use super::*;
use gpui::{AppContext, Modifiers, TestAppContext, VisualTestContext};
use std::{cell::RefCell, rc::Rc};

#[gpui::test]
fn deferred_text_keeps_drafts_and_ignores_stale_replies(cx: &mut TestAppContext) {
    let (handle, control) =
        crate::tests::open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    control.update(cx, |control, cx| {
        assert!(control.set_external_display(0.0, "Unity dB", cx).is_err());
        control.set_deferred_text(true, cx);
        let mut style = crate::tests::STYLE;
        style.text_case = crate::TextCase::Lowercase;
        control.set_external_style(style, cx).unwrap();
        control.set_external_display(0.0, "Unity dB", cx).unwrap();
    });
    let requests = Rc::new(RefCell::new(Vec::<NumericTextRequest>::new()));
    let events = Rc::new(RefCell::new(Vec::<NumericControlEvent>::new()));
    let observed_requests = Rc::clone(&requests);
    let observed_events = Rc::clone(&events);
    let _request_subscription = cx.update(|cx| {
        cx.observe(&control, move |control, cx| {
            if let Some(request) =
                control.update(cx, |control, _| control.take_deferred_text_request())
            {
                observed_requests.borrow_mut().push(request);
            }
        })
    });
    let _event_subscription = cx.update(|cx| {
        cx.subscribe(&control, move |_, event: &NumericControlEvent, _| {
            observed_events.borrow_mut().push(event.clone());
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert_eq!(
        control.read_with(cx, |control, _| control.value_string()),
        "Unity dB"
    );
    let value = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    assert_eq!(
        control.read_with(cx, |control, _| control.display_text().to_owned()),
        "Unity dB"
    );
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Double dB");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let first = requests.borrow()[0].clone();
    assert_eq!(first.text, "Double dB");
    assert!(events.borrow().is_empty());
    assert_eq!(control.read_with(cx, |control, _| control.value()), 0.0);
    control.update(cx, |control, cx| {
        control
            .set_external_display(0.0, "Refreshed dB", cx)
            .unwrap();
        assert_eq!(control.display_text(), "Double dB");
        assert!(control.prepare_finish_edit_for_owner(cx).is_err());
    });
    assert!(
        control
            .update(cx, |control, cx| control.finish_edit(cx))
            .is_err()
    );
    assert_eq!(
        requests.borrow().len(),
        1,
        "blur cannot duplicate the pending request"
    );
    control.update(cx, |control, cx| {
        assert!(
            control
                .resolve_deferred_text(&first.id, None, None, cx)
                .unwrap()
        );
        assert!(control.is_editing());
        assert!(control.invalid_text);
        assert_eq!(control.display_text(), "Double dB");
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let second = requests.borrow()[1].clone();
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("Half dB");
    control.update(cx, |control, cx| {
        assert!(
            !control
                .resolve_deferred_text(&second.id, Some(0.5), None, cx)
                .unwrap()
        );
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let third = requests.borrow()[2].clone();
    assert_eq!(third.text, "Half dB");
    control.update(cx, |control, cx| {
        assert!(
            control
                .resolve_deferred_text(&third.id, Some(0.5), Some("Half dB"), cx)
                .unwrap()
        );
        assert!(
            !control
                .resolve_deferred_text(&third.id, Some(0.7), None, cx)
                .unwrap()
        );
        assert!(!control.is_editing());
        assert_eq!(control.value(), 0.5);
        assert_eq!(control.value_string(), "Half dB");
    });
    visual.run_until_parked();
    assert_eq!(
        events
            .borrow()
            .iter()
            .map(|event| event.phase)
            .collect::<Vec<_>>(),
        [
            NumericEventPhase::Begin,
            NumericEventPhase::Change,
            NumericEventPhase::Commit
        ]
    );
}

#[gpui::test]
fn deferred_text_cancel_revokes_reply_and_unedited_blur_is_not_an_edit(cx: &mut TestAppContext) {
    let (handle, control) =
        crate::tests::open_numeric_control(cx, NumericControlKind::HorizontalSlider);
    control.update(cx, |control, cx| {
        control.set_deferred_text(true, cx);
        control.set_external_display(0.0, "Unity", cx).unwrap();
    });
    let requests = Rc::new(RefCell::new(Vec::<NumericTextRequest>::new()));
    let observed = Rc::clone(&requests);
    let _subscription = cx.update(|cx| {
        cx.observe(&control, move |control, cx| {
            if let Some(request) =
                control.update(cx, |control, _| control.take_deferred_text_request())
            {
                observed.borrow_mut().push(request);
            }
        })
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let value = visual.debug_bounds("test-control-value").unwrap();
    visual.simulate_click(value.center(), Modifiers::default());
    control.update(cx, |control, cx| {
        let unchanged = control.prepare_finish_edit_for_owner(cx).unwrap().unwrap();
        assert!(!unchanged.explicit_edit);
        control.accept_finish_edit_for_owner(unchanged, cx);
    });
    assert!(requests.borrow().is_empty());
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("-3 dB");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    let request = requests.borrow()[0].clone();
    visual.simulate_keystrokes("escape");
    control.update(cx, |control, cx| {
        assert!(
            !control
                .resolve_deferred_text(&request.id, Some(0.5), None, cx)
                .unwrap()
        );
        assert!(!control.is_editing());
        assert_eq!(control.value(), 0.0);
        assert_eq!(control.value_string(), "Unity");
    });
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    let unicode = "漢".repeat(100);
    visual.simulate_input(&unicode);
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(requests.borrow()[1].text, unicode);
    let replaced = requests.borrow()[1].id.clone();
    control.update(cx, |control, cx| {
        // The owner uses cancellation when its source is replaced or removed.
        control.cancel(cx);
        assert!(
            !control
                .resolve_deferred_text(&replaced, Some(0.3), None, cx)
                .unwrap()
        );
        assert_eq!(control.value(), 0.0);
    });
    visual.simulate_click(value.center(), Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("draft");
    control.update(cx, |control, _| control.marked = Some(0..1));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(requests.borrow().len(), 2, "marked input is not submitted");
    control.update(cx, |control, cx| {
        assert!(control.finish_edit(cx).is_err());
        assert!(control.prepare_finish_edit_for_owner(cx).is_err());
        control.marked = None;
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(requests.borrow()[2].text, "draft");
}

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
