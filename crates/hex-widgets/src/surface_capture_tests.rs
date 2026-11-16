//! Actual normalized surface controls share pointer isolation while retaining their edit semantics.
use crate::{BarEditor, BarEditorEvent, BarEditorPhase, ColorPicker, ColorPickerConfig, RgbColor};
use gpui::{
    Context, CursorStyle, Entity, IntoElement, Modifiers, MouseButton, Render, ScrollDelta,
    ScrollWheelEvent, TestAppContext, VisualTestContext, Window, div, point, prelude::*, px, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone)]
enum Surface {
    Bars(Entity<BarEditor>),
    Color(Entity<ColorPicker>),
}
impl Surface {
    fn view(&self) -> gpui::AnyView {
        match self {
            Self::Bars(v) => v.clone().into(),
            Self::Color(v) => v.clone().into(),
        }
    }
}
struct Root {
    surface: Surface,
    visible: bool,
    actions: Rc<Cell<usize>>,
}
impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let down = self.actions.clone();
        let right = self.actions.clone();
        let up = self.actions.clone();
        let right_up = self.actions.clone();
        let wheel = self.actions.clone();
        let mut layout = div().w(px(320.)).h(px(140.));
        div()
            .size_full()
            .when(self.visible, |d| {
                d.child(self.surface.view().cached(layout.style().clone()))
            })
            .child(
                div()
                    .id("other")
                    .absolute()
                    .left(px(360.))
                    .top_0()
                    .w(px(100.))
                    .h(px(140.))
                    .cursor(CursorStyle::Crosshair)
                    .on_mouse_down(MouseButton::Left, move |_, _, _| down.set(down.get() + 1))
                    .on_mouse_down(MouseButton::Right, move |_, _, _| {
                        right.set(right.get() + 1)
                    })
                    .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                    .on_mouse_up(MouseButton::Right, move |_, _, _| {
                        right_up.set(right_up.get() + 1)
                    })
                    .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
            )
    }
}
fn open(
    cx: &mut TestAppContext,
    color: bool,
) -> (gpui::WindowHandle<Root>, Surface, Rc<Cell<usize>>) {
    let actions = Rc::new(Cell::new(0));
    let handle = cx.add_window(|_, cx| Root {
        surface: if color {
            Surface::Color(cx.new(|cx| {
                ColorPicker::from_config(
                    cx,
                    ColorPickerConfig::new(
                        "surface",
                        RgbColor::new(255, 0, 0),
                        0.,
                        crate::tests::STYLE,
                    )
                    .unwrap(),
                )
            }))
        } else {
            Surface::Bars(cx.new(|cx| BarEditor::new(cx, "surface", crate::tests::STYLE)))
        },
        visible: true,
        actions: actions.clone(),
    });
    let surface = handle.read_with(cx, |r, _| r.surface.clone()).unwrap();
    (handle, surface, actions)
}

#[gpui::test]
fn normalized_surfaces_isolate_other_buttons_wheel_and_outside_release(cx: &mut TestAppContext) {
    for kind in 0..3 {
        let (handle, surface, actions) = open(cx, kind != 0);
        let mut v = VisualTestContext::from_window(*handle, cx);
        v.simulate_resize(size(px(500.), px(180.)));
        v.run_until_parked();
        let selector = match kind {
            0 => "surface",
            1 => "surface-field",
            _ => "surface-hue",
        };
        let bounds = v.debug_bounds(selector).unwrap();
        let start = if kind == 2 {
            // The already selected hue must still acquire a live cached witness.
            point(bounds.left() + px(1.), bounds.center().y)
        } else {
            bounds.center()
        };
        let other = point(px(400.), px(70.));
        let button = if kind == 0 {
            MouseButton::Right
        } else {
            MouseButton::Left
        };
        let secondary = if kind == 0 {
            MouseButton::Left
        } else {
            MouseButton::Right
        };
        let modifiers = Modifiers {
            shift: true,
            ..Default::default()
        };
        let original_hue = if kind == 2 {
            match &surface {
                Surface::Color(p) => Some(p.read_with(cx, |p, _| p.hsv())),
                _ => None,
            }
        } else {
            None
        };
        v.simulate_mouse_down(start, button, modifiers);
        v.run_until_parked();
        if let (Some(original), Surface::Color(p)) = (original_hue, &surface) {
            assert_eq!(
                p.read_with(cx, |p, _| p.hsv()),
                original,
                "stationary hue press must not need a color change"
            );
        }
        v.simulate_mouse_down(other, secondary, modifiers);
        v.simulate_event(ScrollWheelEvent {
            position: other,
            delta: ScrollDelta::Pixels(point(px(0.), px(20.))),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "surface {kind} leaked input before motion"
        );
        v.simulate_mouse_move(other, Some(button), modifiers);
        v.run_until_parked();
        assert_eq!(
            v.rendered_cursor_style(),
            Some(CursorStyle::Arrow),
            "surface {kind} lost its cursor"
        );
        v.update(|window, cx| {
            window
                .root::<Root>()
                .unwrap()
                .unwrap()
                .update(cx, |_, cx| cx.notify())
        });
        v.run_until_parked();
        v.simulate_mouse_up(other, button, modifiers);
        v.simulate_mouse_up(other, secondary, modifiers);
        v.run_until_parked();
        assert_eq!(actions.get(), 0, "surface {kind} leaked an outside release");
        assert_eq!(v.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        v.simulate_mouse_down(other, MouseButton::Left, Modifiers::default());
        v.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
        assert_eq!(
            actions.get(),
            2,
            "surface {kind} did not restore normal input"
        );
        if let Surface::Color(picker) = surface {
            assert!(picker.read_with(cx, |p, _| p.hsv().hue.is_finite()));
        }
    }
}

#[gpui::test]
fn surface_capture_loss_cancels_bar_intents_and_keeps_live_color(cx: &mut TestAppContext) {
    for color in [false, true] {
        for terminal in 0..3 {
            let (handle, surface, actions) = open(cx, color);
            let bar_events = Rc::new(RefCell::new(Vec::new()));
            let colors = Rc::new(Cell::new(0));
            let _subscription = match &surface {
                Surface::Bars(bars) => {
                    let events = bar_events.clone();
                    cx.update(|cx| {
                        cx.subscribe(bars, move |_, e: &BarEditorEvent, _| {
                            events.borrow_mut().push(e.phase)
                        })
                    })
                }
                Surface::Color(picker) => {
                    let colors = colors.clone();
                    cx.update(|cx| {
                        cx.subscribe(picker, move |picker, e: &crate::ColorPickerEvent, cx| {
                            colors.set(colors.get() + 1);
                            // The real theme owner echoes canonical RGB after every live change.
                            picker.update(cx, |p, cx| p.set_external_color(e.color, e.hsv.hue, cx));
                        })
                    })
                }
            };
            let mut v = VisualTestContext::from_window(*handle, cx);
            v.simulate_resize(size(px(500.), px(180.)));
            v.update(|w, _| w.activate_window());
            v.run_until_parked();
            let start = v
                .debug_bounds(if color { "surface-field" } else { "surface" })
                .unwrap()
                .center();
            let moved = start + point(px(20.), px(10.));
            v.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            v.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
            v.run_until_parked();
            let retained = match &surface {
                Surface::Color(p) => Some(p.read_with(cx, |p, _| p.color())),
                _ => None,
            };
            let color_count = colors.get();
            if color {
                assert!(
                    color_count >= 2,
                    "canonical echoes must preserve the held color gesture"
                );
            }
            match terminal {
                0 => v.simulate_keystrokes("escape"),
                1 => v.deactivate_window(),
                _ => v.update(|w, cx| {
                    w.root::<Root>().unwrap().unwrap().update(cx, |r, cx| {
                        r.visible = false;
                        cx.notify();
                    })
                }),
            }
            v.run_until_parked();
            if !color {
                assert_eq!(
                    bar_events
                        .borrow()
                        .iter()
                        .filter(|p| **p == BarEditorPhase::Cancel)
                        .count(),
                    1,
                    "terminal {terminal}"
                );
                assert!(!bar_events.borrow().contains(&BarEditorPhase::Commit));
            }
            let other = point(px(400.), px(70.));
            v.simulate_mouse_move(other, Some(MouseButton::Left), Modifiers::default());
            v.simulate_mouse_up(other, MouseButton::Left, Modifiers::default());
            v.run_until_parked();
            assert_eq!(
                actions.get(),
                0,
                "loss terminal {terminal} leaked its release"
            );
            if let Surface::Color(p) = &surface {
                assert_eq!(
                    Some(p.read_with(cx, |p, _| p.color())),
                    retained,
                    "live color is retained on capture loss"
                );
                assert_eq!(
                    colors.get(),
                    color_count,
                    "cancelled pointer must stop editing color"
                );
            }
        }
    }
}

/// CPU-only manual comparison of actual held controls through complete input,
/// notification and repaint turns. Setup/release, subscriptions, canonical owners,
/// native graphics/audio and destruction are excluded.
#[gpui::test]
#[ignore = "manual optimized surface input/CPU-scene profile; no native acceptance"]
fn profile_surface_capture(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_SURFACE_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for kind in 0..3 {
        let (handle, _, _) = open(cx, kind != 0);
        let mut v = VisualTestContext::from_window(*handle, cx);
        v.simulate_resize(size(px(500.), px(180.)));
        v.run_until_parked();
        let bounds = v
            .debug_bounds(match kind {
                0 => "surface",
                1 => "surface-field",
                _ => "surface-hue",
            })
            .unwrap();
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            v.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
            v.run_until_parked();
            let start = std::time::Instant::now();
            let mut at = bounds.center();
            for step in 0..8 {
                at = bounds.origin
                    + point(
                        bounds.size.width * (0.2 + ((round + step) % 7) as f32 * 0.1),
                        bounds.size.height * 0.4,
                    );
                // Match native event boundaries: flush the model notification
                // before its completed scene instead of replaying a stale cache.
                v.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::default());
                v.run_until_parked();
            }
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.;
            v.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
            v.run_until_parked();
            if round >= warmup {
                timings.push(elapsed);
            }
        }
        timings.sort_by(f64::total_cmp);
        let percentile = |p: usize| timings[(timings.len() - 1) * p / 100];
        let scale = v.update(|w, _| w.scale_factor());
        println!(
            "SURFACE_PROFILE kind={kind} size=500x180 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 moves_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
