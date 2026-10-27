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
    unrelated: usize,
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
            .child(
                div()
                    .absolute()
                    .left(px(240.))
                    .top_0()
                    .w(px(80.))
                    .h(px(80.))
                    .cursor(gpui::CursorStyle::Crosshair)
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|v, _, _, _| v.unrelated += 1),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|v, _, _, _| v.unrelated += 1),
                    )
                    .on_scroll_wheel(cx.listener(|v, _, _, _| v.unrelated += 1)),
            )
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
        unrelated: 0,
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
            assert_eq!(v.cancelled, Some(SplitterCancel::CaptureLost));
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

#[gpui::test]
fn held_splitter_isolates_unrelated_input_and_restores_cursor(cx: &mut TestAppContext) {
    for axis in [SplitterAxis::Horizontal, SplitterAxis::Vertical] {
        let (mut visual, view) = open(cx, axis);
        let start = visual.debug_bounds("splitter").unwrap().center();
        let other = point(px(270.), px(30.));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let stranger = visual.update(|_, cx| cx.new(|_| ()));
        assert!(!visual.update(|w, cx| w.cancel_pointer_capture_for(stranger.entity_id(), cx)));
        assert!(visual.update(|w, _| w.is_pointer_dragging()));
        visual.simulate_mouse_down(other, MouseButton::Right, Modifiers::default());
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: other,
            delta: gpui::ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(view.read_with(&visual, |v, _| v.unrelated), 0);
        visual.simulate_mouse_move(other, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(match axis {
                SplitterAxis::Horizontal => gpui::CursorStyle::ResizeLeftRight,
                SplitterAxis::Vertical => gpui::CursorStyle::ResizeUpDown,
            })
        );
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(other, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        view.read_with(&visual, |v, _| {
            assert_eq!(v.unrelated, 0);
            assert_eq!(v.phases, [1, 1, 0, 1]);
            assert_eq!(v.value, 80.);
            assert_eq!(v.cancelled, Some(SplitterCancel::Escape));
        });
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(gpui::CursorStyle::Crosshair)
        );
        // Rejected starts must leave neither a sizing snapshot nor an input lease.
        view.update(&mut visual, |v, cx| {
            v.enabled = false;
            cx.notify();
        });
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        visual.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
        assert_eq!(view.read_with(&visual, |v, _| v.phases), [1, 1, 0, 1]);
    }
}

/// CPU input/paint comparison with the actual handle and host-owned snapshot.
#[gpui::test]
#[ignore = "manual optimized splitter profile; no native acceptance"]
fn profile_splitter_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_SPLITTER_POINTER_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for axis in [SplitterAxis::Horizontal, SplitterAxis::Vertical] {
        let (mut visual, view) = open(cx, axis);
        let start = visual.debug_bounds("splitter").unwrap().center();
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(view.read_with(&visual, |v, _| v.gesture.is_some()));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = px(((round + step) % 7) as f32 * 2. - 6.);
                at = start
                    + match axis {
                        SplitterAxis::Horizontal => point(displacement, px(0.)),
                        SplitterAxis::Vertical => point(px(0.), displacement),
                    };
                visual.simulate_mouse_move(at, MouseButton::Left, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "SPLITTER_POINTER_PROFILE axis={axis:?} size=320x240 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
