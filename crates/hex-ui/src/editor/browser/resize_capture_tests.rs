//! Native divider input using the existing browser preferences fixture.
use super::*;
use gpui::{CursorStyle, Render, ScrollDelta, ScrollWheelEvent, div, prelude::*};
use std::cell::Cell;
struct Host {
    editor: gpui::Entity<EditorView>,
    actions: Rc<Cell<usize>>,
    x: gpui::Pixels,
}
impl Render for Host {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let editor = self.editor.clone();
        let down = self.actions.clone();
        let up = self.actions.clone();
        let wheel = self.actions.clone();
        div().size_full().child(editor).child(
            div()
                .absolute()
                .left(self.x - px(20.))
                .top_0()
                .w(px(40.))
                .h(px(24.))
                .cursor(CursorStyle::Crosshair)
                .on_mouse_down(MouseButton::Right, move |_, _, _| down.set(down.get() + 1))
                .on_mouse_up(MouseButton::Left, move |_, _, _| up.set(up.get() + 1))
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1)),
        )
    }
}
struct Fixture {
    visual: VisualTestContext,
    editor: gpui::Entity<EditorView>,
    _directory: PreferencesTestDirectory,
    actions: Rc<Cell<usize>>,
    start: gpui::Point<gpui::Pixels>,
    outside: gpui::Point<gpui::Pixels>,
}
fn open(cx: &mut TestAppContext, selector: &'static str) -> Fixture {
    let (handle, editor, directory) = super::fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let start = visual.debug_bounds(selector).unwrap().center();
    let outside = point(start.x + px(35.), px(10.));
    let actions = Rc::new(Cell::new(0));
    visual.update(|w, cx| {
        w.replace_root(cx, |_, _| Host {
            editor: editor.clone(),
            actions: actions.clone(),
            x: outside.x,
        });
    });
    visual.run_until_parked();
    Fixture {
        visual,
        editor,
        _directory: directory,
        actions,
        start,
        outside,
    }
}
#[gpui::test]
fn browser_divider_capture_isolates_input_and_restores_column_cursor(cx: &mut TestAppContext) {
    for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
        let Fixture {
            mut visual,
            editor,
            _directory,
            actions,
            start,
            outside,
        } = open(cx, selector);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        assert!(active(&editor, &mut visual, selector));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        visual.simulate_event(ScrollWheelEvent {
            position: outside,
            delta: ScrollDelta::Lines(point(0., 1.)),
            ..Default::default()
        });
        assert_eq!(
            actions.get(),
            0,
            "{selector} must isolate input before motion"
        );
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            visual.rendered_cursor_style(),
            Some(CursorStyle::ResizeColumn)
        );
        assert_eq!(
            visual.rendered_background(paint_selector(selector)),
            Some(palette(crate::theme::PaletteRole::Secondary))
        );
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Right, Modifiers::default());
        visual.run_until_parked();
        assert!(!active(&editor, &mut visual, selector));
        assert_eq!(actions.get(), 0);
        assert!(!visual.update(|w, _| w.is_pointer_dragging()));
        assert_eq!(visual.rendered_cursor_style(), Some(CursorStyle::Crosshair));
        visual.simulate_mouse_down(outside, MouseButton::Right, Modifiers::default());
        assert_eq!(actions.get(), 1);
    }
}
#[gpui::test]
fn browser_divider_capture_loss_keeps_width_and_blocks_stale_release(cx: &mut TestAppContext) {
    for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
        for loss in ["button", "escape", "focus", "hide", "invalid"] {
            let Fixture {
                mut visual,
                editor,
                _directory,
                actions,
                start,
                outside,
            } = open(cx, selector);
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let width = visual
                .debug_bounds(if selector == "browser-sidebar-resize" {
                    "browser-sidebar"
                } else {
                    "browser-toolbar"
                })
                .unwrap()
                .size
                .width;
            match loss {
                "button" => visual.simulate_mouse_move(
                    outside,
                    Some(MouseButton::Right),
                    Modifiers::default(),
                ),
                "escape" => visual.simulate_keystrokes("escape"),
                "focus" => visual.update(|w, cx| {
                    editor.update(cx, |v, _| v.focus.focus(w));
                    w.dispatch_input(
                        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position: outside + point(px(200.), px(0.)),
                            pressed_button: Some(MouseButton::Left),
                            modifiers: Modifiers::default(),
                        }),
                        cx,
                    );
                    w.dispatch_input(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            position: outside,
                            button: MouseButton::Left,
                            modifiers: Modifiers::default(),
                            click_count: 1,
                        }),
                        cx,
                    );
                }),
                "hide" => visual.simulate_keystrokes("cmd-alt-b"),
                "invalid" => visual.simulate_mouse_move(
                    point(px(f32::NAN), outside.y),
                    MouseButton::Left,
                    Modifiers::default(),
                ),
                _ => unreachable!(),
            }
            visual.run_until_parked();
            if loss != "focus" {
                visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
            }
            visual.run_until_parked();
            assert!(
                !active(&editor, &mut visual, selector),
                "{selector} loss={loss}"
            );
            assert_eq!(actions.get(), 0, "{selector} loss={loss}");
            assert!(!visual.update(|w, _| w.is_pointer_dragging()));
            if loss == "hide" {
                visual.simulate_keystrokes("cmd-alt-b");
            }
            assert_eq!(
                visual
                    .debug_bounds(if selector == "browser-sidebar-resize" {
                        "browser-sidebar"
                    } else {
                        "browser-toolbar"
                    })
                    .unwrap()
                    .size
                    .width,
                width,
                "cancellation keeps the last width: {selector} loss={loss}"
            );
        }
    }
}

/// Actual browser scene through eight completed input/repaint turns per sample.
#[gpui::test]
#[ignore = "manual optimized browser-divider profile; no native acceptance"]
fn profile_browser_divider_pointer(cx: &mut TestAppContext) {
    let smoke = std::env::var_os("HEX_BROWSER_DIVIDER_PROFILE_SMOKE").is_some_and(|v| v == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 128 };
    for selector in ["browser-sidebar-resize", "browser-resize-handle"] {
        let Fixture {
            mut visual,
            editor,
            _directory,
            ..
        } = open(cx, selector);
        visual.simulate_resize(gpui::size(px(1280.), px(800.)));
        visual.run_until_parked();
        let width = editor.read_with(&visual, |v, _| v.browser.width);
        let mut timings = Vec::with_capacity(samples);
        for round in 0..warmup + samples {
            editor.update(&mut visual, |v, cx| {
                assert!(!v.browser.resizing && v.browser.layout.sidebar_drag.is_none());
                // Reset only disposable view sizing outside the measured scope;
                // no canonical edit or worker request belongs to this gesture.
                v.browser.width = width;
                v.browser.layout = Default::default();
                cx.notify();
            });
            visual.run_until_parked();
            let start = visual.debug_bounds(selector).unwrap().center();
            visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            assert!(active(&editor, &mut visual, selector));
            let clock = std::time::Instant::now();
            let mut at = start;
            for step in 0..8 {
                at = start + point(px(((round + step) % 7) as f32 * 2. - 6.), px(0.));
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
            "BROWSER_DIVIDER_PROFILE selector={selector} size=1280x800 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} completed_updates_per_sample=8 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
            percentile(50),
            percentile(95),
            percentile(99),
            percentile(100)
        );
    }
}
