//! Bounded local owner for the production rectangle primitives. Two rectangles,
//! no musical model, worker, timer or resource lifecycle. Pending paint advances
//! only through an explicit accept/refuse action, so repaints are inspectable.
use crate::theme;
use gpui::{
    Bounds, Context, FocusHandle, Focusable, Modifiers, MouseButton, Pixels, Render, Subscription,
    Window, div, prelude::*, px, rgb,
};
use hex_widgets::{
    RectangleCapture, RectangleEdges, RectanglePart, RectangleSelection, RectangleSelectionPolicy,
    rectangle_body, rectangle_edge, rectangle_surface,
};

pub(super) struct RectangleDemo {
    pub section: bool,
    id: String,
    focus: FocusHandle,
    blur: Option<Subscription>,
    bounds: Option<Bounds<Pixels>>,
    canonical: [[f32; 2]; 2],
    preview: [[f32; 2]; 2],
    capture: Option<RectangleCapture<usize>>,
    pending: Option<[[f32; 2]; 2]>,
    selected: [bool; 2],
    enabled: bool,
    status: &'static str,
}
impl RectangleDemo {
    pub fn new(section: bool, id: String, cx: &Context<Self>) -> Self {
        let canonical = if section {
            [[0.1, 0.5], [0.5, 0.9]]
        } else {
            [[0.1, 0.5], [0.7, 0.71]]
        };
        Self {
            section,
            id,
            focus: cx.focus_handle(),
            blur: None,
            bounds: None,
            canonical,
            preview: canonical,
            capture: None,
            pending: None,
            selected: [false; 2],
            enabled: true,
            status: "ready",
        }
    }
    fn edges(&self) -> RectangleEdges {
        RectangleEdges {
            width: if self.section { 5.0 } else { 4.0 },
            minimum_body: if self.section { 0.0 } else { 1.0 },
        }
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(capture) = self.capture.take() {
            let _ = capture.cancel();
            self.preview = self.canonical;
            self.status = "canceled held input";
            cx.notify();
        }
    }
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.cancel(cx);
        self.enabled = enabled;
        cx.notify();
    }
    fn begin(
        &mut self,
        x: f32,
        y: f32,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.enabled || self.pending.is_some() {
            return false;
        }
        let Some(bounds) = self.bounds else {
            return false;
        };
        let left = f32::from(bounds.left());
        let width = f32::from(bounds.size.width);
        if y < f32::from(bounds.top()) || y >= f32::from(bounds.bottom()) {
            return false;
        }
        let mut hit = None;
        if self.section
            && self.canonical[0][1] == self.canonical[1][0]
            && RectangleEdges::seam_hit(x, left + width * self.canonical[0][1], 1.0)
        {
            hit = Some((0, RectanglePart::Seam));
        }
        if hit.is_none() {
            for (index, [start, end]) in self.canonical.into_iter().enumerate() {
                // Match the displayed minimum width of tiny clips.
                let start = left + start * width;
                let end = (left + end * width).max(start + 3.0);
                if let Some(part) = self.edges().hit(x, start, end) {
                    if !self.section
                        && part == RectanglePart::Body
                        && y > f32::from(bounds.top()) + 14.0
                    {
                        continue;
                    }
                    hit = Some((index, part));
                    break;
                }
            }
        }
        let Some((index, part)) = hit else {
            return false;
        };
        let policy = if self.section {
            RectangleSelectionPolicy::Toggle
        } else {
            RectangleSelectionPolicy::Range
        };
        match policy.intent(modifiers) {
            RectangleSelection::Toggle => self.selected[index] = !self.selected[index],
            RectangleSelection::Range { additive } => {
                if !additive {
                    self.selected = [false; 2];
                }
                self.selected.fill(true);
            }
            RectangleSelection::Replace => {
                self.selected = [false; 2];
                self.selected[index] = true;
            }
        }
        self.capture = RectangleCapture::begin(index, part, x, true);
        self.preview = self.canonical;
        self.status = "held preview";
        self.focus.focus(window);
        cx.notify();
        true
    }
    fn change(&mut self, x: f32, cx: &mut Context<Self>) -> bool {
        let Some(capture) = self.capture.as_mut() else {
            return false;
        };
        let Some(intent) = capture.update(x) else {
            return true;
        };
        let Some(bounds) = self.bounds else {
            return true;
        };
        let width = f32::from(bounds.size.width);
        if !width.is_finite() || width <= 0.0 {
            return true;
        }
        let delta = intent.delta_x / width;
        let [start, end] = self.canonical[intent.key];
        let minimum = (1.0 / width).min(end - start);
        self.preview = self.canonical;
        match intent.part {
            RectanglePart::Body => {
                let delta = delta.clamp(-start, 1.0 - end);
                self.preview[intent.key] = [start + delta, end + delta];
            }
            RectanglePart::Leading => {
                self.preview[intent.key][0] = (start + delta).clamp(0.0, end - minimum)
            }
            RectanglePart::Trailing => {
                self.preview[intent.key][1] = (end + delta).clamp(start + minimum, 1.0)
            }
            RectanglePart::Seam => {
                let edge = (end + delta).clamp(start + minimum, self.canonical[1][1] - minimum);
                self.preview[0][1] = edge;
                self.preview[1][0] = edge;
            }
        }
        cx.notify();
        true
    }
    fn finish(&mut self, x: f32, cx: &mut Context<Self>) -> bool {
        if self.capture.is_none() {
            return false;
        }
        self.change(x, cx);
        if self
            .capture
            .take()
            .and_then(|capture| capture.finish(x))
            .is_some()
            && self.preview != self.canonical
        {
            self.pending = Some(self.preview);
            self.status = "pending · enter accepts · backspace refuses";
        } else {
            self.preview = self.canonical;
            self.status = "ready";
        }
        cx.notify();
        true
    }
    fn settle(&mut self, accept: bool, cx: &mut Context<Self>) {
        if let Some(display) = self.pending.take() {
            if accept {
                self.canonical = display;
            }
            self.preview = self.canonical;
            self.status = if accept { "accepted" } else { "refused" };
            cx.notify();
        }
    }
}
impl Focusable for RectangleDemo {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for RectangleDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.blur.is_none() {
            let focus = self.focus.clone();
            self.blur = Some(cx.on_blur(&focus, window, |view, _, cx| view.cancel(cx)));
        }
        let style = theme::control_style();
        let display = self.pending.unwrap_or(if self.capture.is_some() {
            self.preview
        } else {
            self.canonical
        });
        let width = self
            .bounds
            .map_or(320.0, |bounds| f32::from(bounds.size.width));
        let mut area = div()
            .relative()
            .w_full()
            .h(px(72.0))
            .bg(rgb(style.surface))
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-area")
            });
        for (index, [start, end]) in display.into_iter().enumerate() {
            let part = RectanglePart::Body;
            let selected = self.selected[index];
            let mut rect = rectangle_surface(start, end - start, 3.0)
                .id(("rectangle", index))
                .debug_selector({
                    let id = self.id.clone();
                    move || format!("{id}-rectangle-{index}")
                })
                .bg(rgb(if selected { style.accent } else { style.border }))
                .opacity(if self.enabled { 1.0 } else { 0.4 })
                .border_1()
                .border_color(rgb(if self.focus.is_focused(window) {
                    style.text
                } else {
                    style.border
                }))
                .cursor(if self.section {
                    part.cursor()
                } else {
                    gpui::CursorStyle::Arrow
                })
                .text_color(rgb(if selected {
                    style.accent_text
                } else {
                    style.text
                }))
                .child(theme::text(if self.section { "section" } else { "clip" }));
            if !self.section {
                rect = rect.child(rectangle_body(-1.0, 14.0));
            }
            let handle = self.edges().handle_width(((end - start) * width).max(3.0));
            for part in [RectanglePart::Leading, RectanglePart::Trailing] {
                rect = rect.child(rectangle_edge(part, handle, -1.0));
            }
            area = area.child(rect);
        }
        if self.section && display[0][1] == display[1][0] {
            area = area.child(
                rectangle_edge(RectanglePart::Seam, 2.0, 0.0)
                    .left(gpui::relative(display[0][1]))
                    .ml(px(-1.0)),
            );
        }
        let entity = cx.entity();
        area = area.child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    entity.update(cx, |view, cx| {
                        if view.bounds != Some(bounds) {
                            view.bounds = Some(bounds);
                            cx.notify();
                        }
                    });
                    let down = entity.clone();
                    window.on_mouse_event(
                        move |event: &gpui::MouseDownEvent, phase, window, cx| {
                            if phase.capture()
                                && event.button == MouseButton::Left
                                && down.update(cx, |view, cx| {
                                    view.begin(
                                        event.position.x.into(),
                                        event.position.y.into(),
                                        event.modifiers,
                                        window,
                                        cx,
                                    )
                                })
                            {
                                cx.stop_propagation();
                            }
                        },
                    );
                    let moved = entity.clone();
                    window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                        if !phase.capture() {
                            return;
                        }
                        let held = moved.update(cx, |view, cx| {
                            if event.pressed_button == Some(MouseButton::Left) {
                                view.change(event.position.x.into(), cx)
                            } else {
                                let held = view.capture.is_some();
                                view.cancel(cx);
                                held
                            }
                        });
                        if held {
                            cx.stop_propagation();
                        }
                    });
                    let up = entity.clone();
                    window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                        if phase.capture()
                            && event.button == MouseButton::Left
                            && up.update(cx, |view, cx| view.finish(event.position.x.into(), cx))
                        {
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
        div()
            .id(gpui::SharedString::from(self.id.clone()))
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => view.cancel(cx),
                    "enter" => view.settle(true, cx),
                    "backspace" => view.settle(false, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(area)
            .child(theme::text(self.status))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("accept")
                            .child(hex_widgets::action(
                                theme::text("accept pending"),
                                theme::action_style(),
                            ))
                            .on_click(cx.listener(|view, _, _, cx| view.settle(true, cx))),
                    )
                    .child(
                        div()
                            .id("refuse")
                            .child(hex_widgets::action(
                                theme::text("refuse pending"),
                                theme::action_style(),
                            ))
                            .on_click(cx.listener(|view, _, _, cx| view.settle(false, cx))),
                    ),
            )
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, point};
    #[gpui::test]
    fn rectangle_fixtures_hold_pending_across_repaints_cancel_and_refuse(cx: &mut TestAppContext) {
        for section in [false, true] {
            let handle =
                cx.add_window(|_, cx| RectangleDemo::new(section, "rectangle-fixture".into(), cx));
            let view = handle.root(cx).unwrap();
            let mut visual = VisualTestContext::from_window(*handle, cx);
            visual.update(|window, _| window.activate_window());
            visual.run_until_parked();
            let bounds = visual
                .debug_bounds("rectangle-fixture-rectangle-0")
                .unwrap();
            let down = point(
                bounds.left() + bounds.size.width * 0.5,
                bounds.top() + px(5.0),
            );
            let target = point(down.x + px(20.0), down.y);
            let before = view.read_with(cx, |view, _| view.canonical);
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(view.capture.is_some());
                assert_ne!(view.preview, before);
                assert_eq!(view.canonical, before);
            });
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            visual.run_until_parked();
            let released = visual
                .debug_bounds("rectangle-fixture-rectangle-0")
                .unwrap();
            view.read_with(cx, |view, _| {
                assert!(view.capture.is_none());
                assert!(view.pending.is_some());
                assert_eq!(view.canonical, before);
            });
            for _ in 0..3 {
                view.update(cx, |_, cx| cx.notify());
                visual.run_until_parked();
                assert_eq!(
                    visual
                        .debug_bounds("rectangle-fixture-rectangle-0")
                        .unwrap(),
                    released
                );
            }
            visual.simulate_keystrokes("escape");
            view.update(cx, |view, cx| view.set_enabled(false, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(
                    view.pending.is_some(),
                    "cancel/disable cannot retire admitted paint"
                )
            });
            view.update(cx, |view, cx| {
                view.settle(false, cx);
                view.set_enabled(true, cx);
            });
            visual.run_until_parked();
            assert_eq!(
                visual
                    .debug_bounds("rectangle-fixture-rectangle-0")
                    .unwrap(),
                bounds
            );
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert!(view.capture.is_none());
                assert_eq!(view.canonical, before);
                assert_eq!(view.preview, before);
            });
            // Lost-button input cancels; a foreign release cannot finish capture.
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Right, Modifiers::default());
            view.read_with(cx, |view, _| assert!(view.capture.is_some()));
            visual.simulate_mouse_move(target, None, Modifiers::default());
            view.read_with(cx, |view, _| assert!(view.capture.is_none()));
            // Acceptance installs exactly the released display, after which a
            // fresh gesture starts from the newly installed geometry.
            visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            let pending = view.read_with(cx, |view, _| view.pending.unwrap());
            view.update(cx, |view, cx| view.settle(true, cx));
            visual.run_until_parked();
            view.read_with(cx, |view, _| {
                assert_eq!(view.canonical, pending);
                assert!(view.pending.is_none());
            });
        }
    }
    #[gpui::test]
    fn rectangle_fixtures_render_same_narrow_edges_seams_and_focus_in_both_themes(
        cx: &mut TestAppContext,
    ) {
        struct Restore(hex_preferences::ThemePreferences);
        impl Drop for Restore {
            fn drop(&mut self) {
                theme::apply_preferences(self.0);
            }
        }
        let _restore = Restore(theme::preferences());
        for light in [false, true] {
            theme::apply_preferences(if light {
                hex_preferences::ThemePreferences {
                    background: hex_preferences::RgbColor::new(225, 229, 238),
                    text: hex_preferences::RgbColor::new(24, 27, 34),
                    primary: hex_preferences::RgbColor::new(145, 25, 45),
                    secondary: hex_preferences::RgbColor::new(16, 70, 145),
                }
            } else {
                hex_preferences::ThemePreferences::default()
            });
            for section in [false, true] {
                let handle = cx
                    .add_window(|_, cx| RectangleDemo::new(section, "rectangle-narrow".into(), cx));
                let view = handle.root(cx).unwrap();
                let mut visual = VisualTestContext::from_window(*handle, cx);
                visual.update(|window, _| window.activate_window());
                visual.simulate_resize(gpui::size(px(240.0), px(240.0)));
                visual.run_until_parked();
                let a = visual.debug_bounds("rectangle-narrow-rectangle-0").unwrap();
                let b = visual.debug_bounds("rectangle-narrow-rectangle-1").unwrap();
                let at = point(a.left() + px(2.0), a.top() + px(20.0));
                visual.simulate_mouse_move(at, None, Modifiers::default());
                visual.run_until_parked();
                assert_eq!(
                    visual.rendered_cursor_style(),
                    Some(RectanglePart::Leading.cursor()),
                    "section={section}, light={light}, a={a:?}, b={b:?}, at={at:?}"
                );
                if section {
                    let at = point(a.right(), a.top() + px(20.0));
                    visual.simulate_mouse_move(at, None, Modifiers::default());
                    visual.run_until_parked();
                    assert_eq!(
                        visual.rendered_cursor_style(),
                        Some(RectanglePart::Seam.cursor())
                    );
                    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
                    visual.simulate_mouse_move(
                        point(at.x + px(10.0), at.y),
                        MouseButton::Left,
                        Modifiers::default(),
                    );
                    view.read_with(cx, |view, _| {
                        assert_eq!(view.capture.unwrap().part, RectanglePart::Seam);
                        assert_eq!(view.preview[0][1], view.preview[1][0]);
                    });
                    view.update(cx, |view, cx| view.set_enabled(false, cx));
                    view.read_with(cx, |view, _| {
                        assert!(view.capture.is_none());
                        assert_eq!(view.preview, view.canonical);
                    });
                } else {
                    assert!((f32::from(b.size.width) - 3.0).abs() < 0.01);
                    let at = point(b.left() + b.size.width * 0.5, b.top() + px(5.0));
                    visual.simulate_mouse_move(at, None, Modifiers::default());
                    visual.run_until_parked();
                    assert_eq!(
                        visual.rendered_cursor_style(),
                        Some(RectanglePart::Body.cursor())
                    );
                    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
                    view.read_with(cx, |view, _| {
                        assert_eq!(view.capture.unwrap().part, RectanglePart::Body)
                    });
                    visual.run_until_parked();
                    assert!(
                        visual.update(|window, cx| view.read(cx).focus.is_focused(window)),
                        "capture focus before blur light={light}"
                    );
                    visual.update(|window, _| window.blur());
                    visual.run_until_parked();
                    view.read_with(cx, |view, _| {
                        assert!(
                            view.capture.is_none(),
                            "blur retires held input light={light}"
                        )
                    });
                }
                view.update(cx, |view, cx| view.set_enabled(false, cx));
                visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
                view.read_with(cx, |view, _| assert!(view.capture.is_none()));
            }
        }
    }
}
