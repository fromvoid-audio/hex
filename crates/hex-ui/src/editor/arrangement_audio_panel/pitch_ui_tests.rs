//! Pitch uses the shared dial/input and ordinary clip-edit receipt.
use super::*;
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
#[gpui::test]
fn pitch_is_a_dial_and_cancel_keeps_canonical_audio_unchanged(cx: &mut TestAppContext) {
    cx.update(hex_widgets::bind_numeric_control_keys);
    let (handle, editor, _, _) = super::super::super::clip_view_ui_test::audio_detail_fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let before = editor.read_with(cx, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let knob = visual
        .debug_bounds("audio-clip-pitch-knob")
        .expect("pitch must use a shared knob");
    let value = visual
        .debug_bounds("audio-clip-pitch-value")
        .expect("pitch keeps editable cents readout");
    assert!(knob.size.width >= px(20.) && knob.size.height >= px(20.));
    assert!(value.size.width > px(0.));
    let down = knob.center();
    let up = down + point(px(0.), px(-16.));
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
    });
    visual.simulate_keystrokes("escape");
    visual.simulate_mouse_up(up, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert!(v.last_enqueued_edit.is_none());
        assert!(!v.audio_clip_controls.has_pending());
    });
}
