//! Manual optimized input-to-CPU-scene measurement for shared pointer hover.
//! The test platform has no native text shaping, GPU presentation or audio.
use super::*;
use gpui::{AppContext, MouseDownEvent, MouseUpEvent, PlatformInput};
use std::time::Instant;

struct Target;
impl Render for Target {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("profile-target")
            .size_full()
            .bg(rgb(0x111111))
            .hover(|style| style.bg(rgb(0xff0000)))
            .on_hover(|_, _, _| {})
    }
}
struct Root(Vec<gpui::Entity<Target>>);
impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.entity();
        let mut root = div().size_full().child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let owner = owner.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase.capture() && event.pressed_button.is_some() {
                            cx.notify(owner.entity_id());
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        );
        for (index, target) in self.0.iter().enumerate() {
            let mut layout = div().size_full();
            root = root.child(
                div()
                    .absolute()
                    .left(px(20. + (index % 8) as f32 * 150.))
                    .top(px(20. + (index / 8) as f32 * 90.))
                    .w(px(149.))
                    .h(px(89.))
                    .child(gpui::AnyView::from(target.clone()).cached(layout.style().clone())),
            );
        }
        root
    }
}
fn position(index: usize) -> gpui::Point<gpui::Pixels> {
    point(
        px(40. + (index % 8) as f32 * 150.),
        px(40. + (index / 8) as f32 * 90.),
    )
}

#[gpui::test]
#[ignore = "manual optimized pointer input/paint profile; headless CPU only"]
fn headless_pointer_hover_profile(cx: &mut TestAppContext) {
    let smoke =
        std::env::var_os("HEX_POINTER_HOVER_PROFILE_SMOKE").is_some_and(|value| value == "1");
    assert!(
        !cfg!(debug_assertions) || smoke,
        "timing requires --release; debug smoke must be explicit"
    );
    let warmup = if smoke { 1 } else { 16 };
    let samples = if smoke { 4 } else { 256 };
    let targets = (0..64).map(|_| cx.new(|_| Target)).collect();
    let handle = cx.add_window(|_, _| Root(targets));
    let mut visual = VisualTestContext::from_window(*handle, cx);
    visual.simulate_resize(size(px(1240.), px(760.)));
    visual.run_until_parked();
    let scale = visual.update(|window, _| window.scale_factor());
    let mut timings = Vec::with_capacity(samples);
    for round in 0..warmup + samples {
        visual.update(|window, cx| {
            window.dispatch_input(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: position(0),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
            window.draw(cx).clear();
            window.dispatch_input(
                PlatformInput::MouseDown(MouseDownEvent {
                    position: position(0),
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear();
        });
        let elapsed = visual.update(|window, cx| {
            let start = Instant::now();
            let mut at = position(0);
            for step in 0..8 {
                at = position(((round + step) * 17 + 1) % 64);
                window.dispatch_input(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: at,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
                window.draw(cx).clear();
            }
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.;
            // Release/reset is outside the measured held-pointer move scope.
            window.dispatch_input(
                PlatformInput::MouseUp(MouseUpEvent {
                    position: at,
                    button: MouseButton::Left,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear();
            elapsed
        });
        visual.run_until_parked();
        if round >= warmup {
            timings.push(elapsed);
        }
    }
    timings.sort_by(f64::total_cmp);
    let percentile = |percent: usize| timings[(timings.len() - 1) * percent / 100];
    println!(
        "POINTER_PROFILE controls=64 cached_children=64 size=1240x760 scale={scale} warmup={warmup} samples={samples} debug_smoke={smoke} moves_per_sample=8 release_frames_per_sample=0 headless_cpu_only=true p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3}",
        percentile(50),
        percentile(95),
        percentile(99),
        percentile(100),
    );
}
