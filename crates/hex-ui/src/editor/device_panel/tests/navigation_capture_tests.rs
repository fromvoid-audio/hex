//! Actual rail input; view-only scroll ownership remains in Navigation.
use super::*;
use gpui::{CursorStyle, Render, ScrollDelta, ScrollWheelEvent, div};
use std::{cell::Cell, rc::Rc};
struct Host {
    editor: gpui::Entity<EditorView>,
    actions: Rc<Cell<usize>>,
}
impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let editor = self.editor.clone();
        let down = self.actions.clone();
        let up = self.actions.clone();
        let middle = self.actions.clone();
        let wheel = self.actions.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left(px(600.))
                .top_0()
                .w(px(80.))
                .h(px(24.))
                .cursor(CursorStyle::Crosshair)
                .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                .on_mouse_up(MouseButton::Middle, move |_, _, _| {
                    middle.set(middle.get() + 1)
                })
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
        )
    }
}
struct Fixture {
    visual: VisualTestContext,
    editor: gpui::Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    start: gpui::Point<Pixels>,
    button: MouseButton,
}
fn open(cx: &mut TestAppContext, overview: bool) -> Fixture {
    cx.update(crate::editor::bind_editor_keys);
    let (service, mut state, track, _, _, _) = actor_utility_fixture();
    for _ in 0..8 {
        service
            .submit(Request::AddNativeDevice {
                track,
                class: NativeDeviceClass::Effect,
                type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
                    "hex.utility",
                )),
                before: None,
            })
            .expect("append utility");
        state = receive_service_event(&service).state;
    }
    let (handle, editor) = actor_editor_window(cx, service, state, track);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.simulate_resize(size(px(880.), px(540.)));
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
        });
        editor.read(cx).focus.focus(w);
    });
    visual.run_until_parked();
    let selector = if overview {
        "device-chain-overview"
    } else {
        "device-chain-scroll"
    };
    let start = visual
        .debug_bounds(selector)
        .expect("navigation bounds")
        .center();
    Fixture {
        visual,
        editor,
        actions,
        start,
        button: if overview {
            MouseButton::Left
        } else {
            MouseButton::Middle
        },
    }
}
#[gpui::test]
fn navigation_capture_isolates_input_and_retains_cursor(cx: &mut TestAppContext) {
    for overview in [false, true] {
        let Fixture {
            mut visual,
            editor,
            actions,
            start,
            button,
        } = open(cx, overview);
        let outside = point(px(640.), px(10.));
        visual.simulate_mouse_down(start, button, Modifiers::default());
        assert!(editor.read_with(&visual, |v, _| v.device_panel.navigation.is_active()));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "navigation must isolate input before motion"
        );
        visual.simulate_mouse_move(outside, button, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(if overview {
                CursorStyle::PointingHand
            } else {
                CursorStyle::ClosedHand
            })
        );
        visual.simulate_mouse_up(outside, button, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(actions.get(), 0);
        assert!(!editor.read_with(&visual, |v, _| v.device_panel.navigation.is_active()));
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
    }
}
#[gpui::test]
fn navigation_capture_loss_keeps_last_offset_and_isolates_late_up(cx: &mut TestAppContext) {
    for overview in [false, true] {
        for loss in [
            "button",
            "focus",
            "invalid",
            "escape",
            "generation",
            "hidden",
        ] {
            let Fixture {
                mut visual,
                editor,
                actions,
                start,
                button,
            } = open(cx, overview);
            let outside = point(px(640.), px(10.));
            let before =
                editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
            visual.simulate_mouse_down(start, button, Modifiers::default());
            let moved = start + point(px(if overview { 15. } else { -25. }), px(0.));
            visual.simulate_mouse_move(moved, button, Modifiers::default());
            visual.run_until_parked();
            let offset =
                editor.read_with(&visual, |v, _| v.device_panel.navigation.scroll.offset());
            match loss {
                "button" => visual.simulate_mouse_move(outside, None, Modifiers::default()),
                "invalid" => visual.simulate_mouse_move(
                    point(px(f32::NAN), px(f32::NAN)),
                    button,
                    Modifiers::default(),
                ),
                "escape" => visual.simulate_keystrokes("escape"),
                "generation" => editor.update(&mut visual, |v, cx| {
                    v.project_generation += 1;
                    cx.notify();
                }),
                "hidden" => editor.update(&mut visual, |v, cx| {
                    v.clip_view_open = false;
                    cx.notify();
                }),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, cx| v.search_browser(w, cx));
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            position: outside,
                            button,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                }),
                _ => unreachable!(),
            }
            visual.run_until_parked();
            if loss != "focus" {
                visual.simulate_mouse_up(outside, button, Modifiers::default());
            }
            visual.run_until_parked();
            assert_eq!(actions.get(), 0, "overview={overview} loss={loss}");
            editor.read_with(&visual, |v, _| {
                assert!(
                    !v.device_panel.navigation.is_active(),
                    "overview={overview} loss={loss}"
                );
                if loss != "generation" {
                    assert_eq!(
                        v.device_panel.navigation.scroll.offset(),
                        offset,
                        "overview={overview} loss={loss}"
                    );
                }
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
                assert!(v.last_enqueued_edit.is_none());
            });
        }
    }
}

/// Real ten-card rail; setup/reset and native delivery excluded.
#[gpui::test]
#[ignore = "manual optimized navigation profile; no native acceptance"]
fn profile_navigation_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_NAVIGATION_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for overview in [false, true] {
        let Fixture {
            mut visual,
            editor,
            start,
            button,
            ..
        } = open(cx, overview);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            visual.update(|w, cx| {
                editor
                    .read(cx)
                    .device_panel
                    .navigation
                    .scroll
                    .set_offset(point(px(0.), px(0.)));
                w.refresh();
            });
            visual.run_until_parked();
            visual.simulate_mouse_down(start, button, Modifiers::default());
            visual.run_until_parked();
            assert!(editor.read_with(&visual, |v, _| v.device_panel.navigation.is_active()));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                let displacement = px(((round + step) % 7) as f32 * 2.);
                at = start
                    + point(
                        if overview {
                            displacement
                        } else {
                            -displacement
                        },
                        px(0.),
                    );
                visual.simulate_mouse_move(at, button, Modifiers::default());
                visual.run_until_parked();
            }
            let elapsed = clock.elapsed().as_secs_f64() * 1_000_000.;
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(at, button, Modifiers::default());
            visual.run_until_parked();
            editor.read_with(&visual, |v, _| {
                assert!(!v.device_panel.navigation.is_active());
                assert!(v.last_enqueued_edit.is_none());
            });
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = visual.update(|w, _| w.scale_factor());
        println!(
            "NAVIGATION_PROFILE overview={overview} size=880x540 scale={scale} cards=10 warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
