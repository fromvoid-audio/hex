use super::*;
use gpui::{FocusHandle, Render, TestAppContext, VisualTestContext, point, size};

struct Fixture {
    config: SplitterHandleConfig,
    gesture: Option<SplitterGesture>,
    original: f32,
    value: f32,
    enabled: bool,
    phases: [usize; 4],
    focus: FocusHandle,
    cancelled: Option<SplitterCancel>,
}
impl Fixture {
    fn input(cx: &Context<Self>) -> SplitterInput<Self> {
        SplitterInput::new(cx.entity(), |view| view.gesture, Self::event)
    }
    fn event(&mut self, event: SplitterEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match event {
            SplitterEvent::Begin(gesture) => {
                if !self.enabled {
                    return false;
                }
                self.original = self.value;
                self.gesture = Some(gesture);
                self.phases[0] += 1;
                self.focus.focus(window);
            }
            SplitterEvent::Change(delta) | SplitterEvent::Commit(delta) => {
                if self.gesture.is_none() {
                    return false;
                }
                self.value = (self.original + delta).clamp(16.0, 200.0);
                if matches!(event, SplitterEvent::Commit(_)) {
                    self.gesture = None;
                    self.phases[2] += 1;
                } else {
                    self.phases[1] += 1;
                }
            }
            SplitterEvent::Cancel(reason) => {
                if self.gesture.take().is_none() {
                    return false;
                }
                self.value = self.original;
                self.cancelled = Some(reason);
                self.phases[3] += 1;
            }
        }
        cx.notify();
        true
    }
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = Self::input(cx);
        let handle = input
            .handle("splitter".into(), self.config)
            .absolute()
            .left(px(100.0))
            .top(px(100.0));
        let handle = match self.config.axis {
            SplitterAxis::Horizontal => handle.h(px(40.0)),
            SplitterAxis::Vertical => handle.w(px(40.0)),
        };
        div()
            .id("host")
            .track_focus(&self.focus)
            .capture_key_down(input.escape_listener())
            .size_full()
            .child(handle)
            .child(input.pointer_layer())
    }
}
fn open(cx: &mut TestAppContext, axis: SplitterAxis) -> (VisualTestContext, Entity<Fixture>) {
    let handle = cx.add_window(|_, cx| Fixture {
        config: SplitterHandleConfig::thin(axis, 0x808080),
        gesture: None,
        original: 80.0,
        value: 80.0,
        enabled: true,
        phases: [0; 4],
        focus: cx.focus_handle(),
        cancelled: None,
    });
    let view = handle.root(cx).unwrap();
    let visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(320.0), px(240.0)));
    visual.run_until_parked();
    (visual, view)
}

#[gpui::test]
fn axes_use_final_outside_release_and_emit_one_terminal_after_repaints(cx: &mut TestAppContext) {
    for axis in [SplitterAxis::Horizontal, SplitterAxis::Vertical] {
        let (mut visual, view) = open(cx, axis);
        let handle = visual.debug_bounds("splitter").unwrap();
        assert_eq!(
            if axis == SplitterAxis::Horizontal {
                handle.size.width
            } else {
                handle.size.height
            },
            px(4.0)
        );
        let start = handle.center();
        let along = |amount| match axis {
            SplitterAxis::Horizontal => start + point(px(amount), px(0.0)),
            SplitterAxis::Vertical => start + point(px(0.0), px(amount)),
        };
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(along(20.0), MouseButton::Left, Modifiers::default());
        view.read_with(&visual, |v, _| assert_eq!(v.value, 100.0));
        // No final move: the release itself lies outside the window and must win.
        visual.simulate_mouse_up(along(500.0), MouseButton::Left, Modifiers::default());
        for _ in 0..3 {
            visual.run_until_parked();
        }
        view.read_with(&visual, |v, _| {
            assert_eq!(v.value, 200.0);
            assert!(v.gesture.is_none());
            assert_eq!(v.phases, [1, 1, 1, 0]);
        });
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(along(-20.0), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(along(-40.0), MouseButton::Left, Modifiers::default());
        view.read_with(&visual, |v, _| {
            assert_eq!(v.value, 200.0);
            assert_eq!(v.cancelled, Some(SplitterCancel::Escape));
            assert_eq!(v.phases, [2, 2, 1, 1]);
        });
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(along(-30.0), None, Modifiers::default());
        visual.simulate_mouse_up(along(-40.0), MouseButton::Left, Modifiers::default());
        view.read_with(&visual, |v, _| {
            assert_eq!(v.value, 200.0);
            assert_eq!(v.cancelled, Some(SplitterCancel::ButtonLost));
            assert_eq!(v.phases, [3, 2, 1, 2]);
        });
    }
}

#[gpui::test]
fn refused_begin_is_inert_and_gesture_rejects_invalid_geometry_and_foreign_windows(
    cx: &mut TestAppContext,
) {
    let (mut visual, view) = open(cx, SplitterAxis::Vertical);
    view.update(&mut visual, |v, cx| {
        v.enabled = false;
        cx.notify();
    });
    let start = visual.debug_bounds("splitter").unwrap().center();
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        start + point(px(0.0), px(50.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
    view.read_with(&visual, |v, _| {
        assert_eq!(v.value, 80.0);
        assert_eq!(v.phases, [0; 4]);
    });
    let gesture = visual.update(|window, _| {
        assert!(
            SplitterGesture::start(SplitterAxis::Vertical, point(px(f32::NAN), px(0.0)), window)
                .is_none()
        );
        let gesture = SplitterGesture::start(
            SplitterAxis::Horizontal,
            point(px(-f32::MAX), px(0.0)),
            window,
        )
        .unwrap();
        assert!(gesture.displacement(point(px(f32::MAX), px(0.0))).is_none());
        gesture
    });
    let (mut other, _) = open(cx, SplitterAxis::Horizontal);
    other.update(|window, _| {
        assert!(matches!(
            gesture.event(start, window, true),
            SplitterEvent::Cancel(SplitterCancel::WindowChanged)
        ))
    });
}

#[test]
fn handle_geometry_is_finite_positive_and_bounded() {
    for thickness in [0.0, -1.0, 65.0, f32::MAX, f32::INFINITY, f32::NAN] {
        assert!(SplitterHandleConfig::new(SplitterAxis::Vertical, thickness, 0).is_none());
    }
    assert!(SplitterHandleConfig::new(SplitterAxis::Horizontal, 64.0, 0).is_some());
}

#[gpui::test]
fn replacement_window_cancels_old_capture_without_a_late_commit(cx: &mut TestAppContext) {
    struct Mirror(Entity<Fixture>);
    impl Render for Mirror {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.clone()
        }
    }
    let (mut first, view) = open(cx, SplitterAxis::Vertical);
    let start = first.debug_bounds("splitter").unwrap().center();
    first.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    first.simulate_mouse_move(
        start + point(px(0.0), px(20.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    let handle = cx.add_window(|_, _| Mirror(view.clone()));
    let mut replacement = VisualTestContext::from_window(*handle, cx);
    replacement.run_until_parked();
    replacement.simulate_mouse_move(
        start + point(px(0.0), px(40.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    first.simulate_mouse_up(
        start + point(px(0.0), px(60.0)),
        MouseButton::Left,
        Modifiers::default(),
    );
    view.read_with(&first, |v, _| {
        assert_eq!(v.value, 80.0);
        assert!(v.gesture.is_none());
        assert_eq!(v.cancelled, Some(SplitterCancel::WindowChanged));
        assert_eq!(v.phases, [1, 1, 0, 1]);
    });
}
