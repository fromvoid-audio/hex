//! Bounded local owner for production rectangle and time-range policies. Two rectangles or one range,
//! no musical model, worker, timer or resource lifecycle. Pending paint advances
//! only through an explicit accept/refuse action, so repaints are inspectable.
use crate::theme;
use gpui::{
    Bounds, Context, FocusHandle, Focusable, Modifiers, MouseButton, Pixels, Render, Subscription,
    Window, div, prelude::*, px, rgb,
};
use hex_widgets::{
    RangeSelectionTarget, RangeSelectionZones, RectangleCapture, RectangleEdges, RectanglePart,
    RectangleSelection, RectangleSelectionPolicy, rectangle_body, rectangle_edge,
    rectangle_surface,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Clip,
    Section,
    TimeRange,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Rectangle(usize),
    PaintRange,
    SectionRange,
}
pub(super) struct RectangleDemo {
    kind: Kind,
    id: String,
    focus: FocusHandle,
    blur: Option<Subscription>,
    bounds: Option<Bounds<Pixels>>,
    canonical: [[f32; 2]; 2],
    preview: [[f32; 2]; 2],
    capture: Option<RectangleCapture<Target>>,
    pending: Option<[[f32; 2]; 2]>,
    range: Option<[f32; 2]>,
    range_before: Option<[f32; 2]>,
    selected: Vec<usize>,
    selection_primary: Option<usize>,
    selection_anchor: Option<usize>,
    enabled: bool,
    status: &'static str,
}
impl RectangleDemo {
    pub fn new(kind: Kind, id: String, cx: &Context<Self>) -> Self {
        let canonical = if kind == Kind::Section {
            [[0.1, 0.5], [0.5, 0.9]]
        } else {
            [[0.1, 0.5], [0.7, 0.71]]
        };
        Self {
            kind,
            id,
            focus: cx.focus_handle(),
            blur: None,
            bounds: None,
            canonical,
            preview: canonical,
            capture: None,
            pending: None,
            range: None,
            range_before: None,
            selected: if kind == Kind::TimeRange {
                vec![0]
            } else {
                Vec::new()
            },
            selection_primary: None,
            selection_anchor: None,
            enabled: true,
            status: "ready",
        }
    }
    fn edges(&self) -> RectangleEdges {
        RectangleEdges {
            width: if self.kind == Kind::Section { 5.0 } else { 4.0 },
            minimum_body: if self.kind == Kind::Section { 0.0 } else { 1.0 },
        }
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(capture) = self.capture.take() {
            if capture.key == Target::SectionRange {
                self.range = self.range_before.take();
            } else {
                self.preview = self.canonical;
            }
            let _ = capture.cancel();
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
        if self.kind == Kind::TimeRange {
            if !x.is_finite() || x < left || x >= left + width {
                return false;
            }
            let Some(zones) = RangeSelectionZones::new(bounds.size.height.into(), 0.0) else {
                return false;
            };
            let [start, end] = self.canonical[0];
            let existing = x >= left + start * width && x < left + end * width;
            let Some(target) = zones.target(y - f32::from(bounds.top()), existing, modifiers.shift)
            else {
                return false;
            };
            let target = match target {
                RangeSelectionTarget::Move => Target::Rectangle(0),
                RangeSelectionTarget::Paint => Target::PaintRange,
            };
            self.capture = RectangleCapture::begin(target, RectanglePart::Body, x, true);
            self.preview = self.canonical;
            self.selected = vec![0];
            self.status = "held range preview";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        let mut hit = if self.kind == Kind::Section {
            let sections = self
                .canonical
                .iter()
                .enumerate()
                .map(|(index, &[start, end])| (index, start, end));
            crate::section_hit::hit(sections, x, |position| Some(left + width * position)).map(
                |hit| match hit {
                    crate::section_hit::Hit::Leading(index) => (index, RectanglePart::Leading),
                    crate::section_hit::Hit::Trailing(index) => (index, RectanglePart::Trailing),
                    crate::section_hit::Hit::Seam(left, _) => (left, RectanglePart::Seam),
                },
            )
        } else {
            None
        };
        if hit.is_none() {
            for (index, [start, end]) in self.canonical.into_iter().enumerate() {
                let start = left + start * width;
                let end = left + end * width;
                if let Some(part) = self.edges().hit(x, start, end) {
                    if self.kind == Kind::Section && part != RectanglePart::Body {
                        continue;
                    }
                    if self.kind == Kind::Clip
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
        let policy = if self.kind == Kind::Section {
            RectangleSelectionPolicy::Section
        } else {
            RectangleSelectionPolicy::Range
        };
        let intent = policy.intent(modifiers);
        let time_range = if self.kind == Kind::Section {
            let Some(time_range) = crate::section_input::routes_to_time_range(
                hit.is_some_and(|(_, part)| part != RectanglePart::Body),
                y,
                f32::from(bounds.top()),
                f32::from(bounds.size.height),
                44.0 * hex_widgets::RANGE_HANDLE_FRACTION,
            ) else {
                return false;
            };
            time_range
        } else {
            false
        };
        if self.kind == Kind::Section
            && intent != RectangleSelection::Replace
            && let Some((index, _)) = hit
        {
            let canonical = self.canonical;
            let selection = crate::section_selection::extend(
                std::mem::take(&mut self.selected),
                self.selection_primary,
                self.selection_anchor,
                index,
                intent == RectangleSelection::Toggle,
                |anchor, clicked| {
                    let low = canonical[anchor][0].min(canonical[clicked][0]);
                    let high = canonical[anchor][0].max(canonical[clicked][0]);
                    let mut order = [0, 1];
                    order.sort_by(|a, b| canonical[*a][0].total_cmp(&canonical[*b][0]));
                    order
                        .into_iter()
                        .filter(|i| low <= canonical[*i][0] && canonical[*i][0] <= high)
                        .collect()
                },
            );
            self.selected = selection.members;
            self.selection_primary = selection.selected;
            self.selection_anchor = Some(selection.anchor);
            self.status = "selected";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        if time_range {
            if !x.is_finite()
                || !left.is_finite()
                || !width.is_finite()
                || width <= 0.0
                || x < left
                || x >= left + width
            {
                return false;
            }
            let anchor = ((x - left) / width).clamp(0.0, 1.0);
            self.capture =
                RectangleCapture::begin(Target::SectionRange, RectanglePart::Body, x, true);
            self.range_before = self.range;
            self.range = Some([anchor, anchor]);
            self.selected.clear();
            self.selection_primary = None;
            self.status = "held range preview";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        let Some((index, part)) = hit else {
            return false;
        };
        match intent {
            RectangleSelection::Toggle => {
                if self.selected.contains(&index) {
                    self.selected.retain(|id| *id != index);
                } else {
                    self.selected.push(index);
                }
            }
            RectangleSelection::Range { additive } => {
                if !additive {
                    self.selected.clear();
                }
                for id in [0, 1] {
                    if !self.selected.contains(&id) {
                        self.selected.push(id);
                    }
                }
            }
            RectangleSelection::Replace => {
                self.selected.clear();
                self.selected.push(index);
            }
        }
        if self.kind == Kind::Section {
            self.selection_primary = Some(index);
            self.selection_anchor = Some(index);
        }
        self.capture = RectangleCapture::begin(Target::Rectangle(index), part, x, true);
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
        if intent.key == Target::SectionRange {
            let left = f32::from(bounds.left());
            let anchor = ((intent.origin_x - left) / width).clamp(0.0, 1.0);
            let end = ((intent.pointer_x - left) / width).clamp(0.0, 1.0);
            self.range = Some([anchor.min(end), anchor.max(end)]);
            cx.notify();
            return true;
        }
        if intent.key == Target::PaintRange {
            let left = f32::from(bounds.left());
            let anchor = ((intent.origin_x - left) / width).clamp(0.0, 1.0);
            let end = ((intent.pointer_x - left) / width).clamp(0.0, 1.0);
            self.preview[0] = [anchor.min(end), anchor.max(end)];
            cx.notify();
            return true;
        }
        let Target::Rectangle(index) = intent.key else {
            return true;
        };
        let [start, end] = self.canonical[index];
        let minimum = (1.0 / width).min(end - start);
        self.preview = self.canonical;
        match intent.part {
            RectanglePart::Body => {
                let delta = delta.clamp(-start, 1.0 - end);
                self.preview[index] = [start + delta, end + delta];
            }
            RectanglePart::Leading => {
                self.preview[index][0] = (start + delta).clamp(0.0, end - minimum)
            }
            RectanglePart::Trailing => {
                self.preview[index][1] = (end + delta).clamp(start + minimum, 1.0)
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
            .is_some_and(|capture| capture.key == Target::SectionRange)
        {
            let completed = self
                .capture
                .take()
                .and_then(|capture| capture.finish(x))
                .is_some();
            if !completed {
                self.range = self.range_before;
            }
            self.range_before = None;
            self.status = if completed {
                "range selected"
            } else {
                "range canceled"
            };
            cx.notify();
            return true;
        }
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
            if self.kind == Kind::TimeRange && index != 0 {
                continue;
            }
            let selected = self.selected.contains(&index);
            let pixel_width = (end - start) * width;
            let identity = if index == 0 {
                hex_project::ElementColor::Cyan
            } else {
                hex_project::ElementColor::Blue
            };
            let surface = if self.kind == Kind::Section {
                crate::section_surface::surface(
                    start,
                    end - start,
                    pixel_width,
                    1.0 / window.scale_factor().max(1.0),
                    identity,
                    selected,
                    false,
                )
                .cursor(RectanglePart::Body.cursor())
                .child(
                    crate::section_surface::header(
                        identity,
                        selected,
                        false,
                        44.0 * hex_widgets::RANGE_HANDLE_FRACTION,
                    )
                    .debug_selector({
                        let id = self.id.clone();
                        move || format!("{id}-section-header-{index}")
                    })
                    .child(theme::text(if selected {
                        "✓ section"
                    } else {
                        "section"
                    })),
                )
            } else {
                rectangle_surface(start, end - start, pixel_width)
                    .bg(rgb(if selected { style.accent } else { style.border }))
                    .border_color(rgb(if self.focus.is_focused(window) {
                        style.text
                    } else {
                        style.border
                    }))
                    .cursor(gpui::CursorStyle::Arrow)
                    .text_color(rgb(if selected {
                        style.accent_text
                    } else {
                        style.text
                    }))
                    .child(theme::text(if self.kind == Kind::Clip {
                        "clip"
                    } else {
                        "time range"
                    }))
            };
            let mut rect = surface
                .id(("rectangle", index))
                .debug_selector({
                    let id = self.id.clone();
                    move || format!("{id}-rectangle-{index}")
                })
                .opacity(if self.enabled { 1.0 } else { 0.4 });
            let border = hex_widgets::rectangle_border_width(pixel_width);
            let handle = self.edges().handle_width(pixel_width);
            if self.kind == Kind::TimeRange
                && let Some(zones) = RangeSelectionZones::new(72.0, 0.0)
            {
                rect = rect.child(rectangle_body(-1.0, zones.handle_height()));
            }
            if self.kind == Kind::Clip {
                rect = rect.child(
                    rectangle_body(-border, 14.0)
                        .left(px(handle - border))
                        .right(px(handle - border)),
                );
            }
            if self.kind != Kind::TimeRange {
                for part in [RectanglePart::Leading, RectanglePart::Trailing] {
                    rect = rect.child(rectangle_edge(part, handle, -border));
                }
            }
            area = area.child(rect);
        }
        if self.kind == Kind::Section
            && let Some([start, end]) = self.range
            && end > start
        {
            area = area.child(
                rectangle_surface(start, end - start, width * (end - start))
                    .debug_selector(|| "section-time-range".into())
                    .bg(rgb(style.accent))
                    .opacity(0.25),
            );
        }
        if self.kind == Kind::Section && display[0][1] == display[1][0] {
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
    fn time_range_body_repaints_while_upper_rail_moves_and_pending_survives_cancel(
        cx: &mut TestAppContext,
    ) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::TimeRange, "range-fixture".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("range-fixture-area").unwrap();
        let at = |x, y| {
            point(
                area.left() + area.size.width * x,
                area.top() + area.size.height * y,
            )
        };
        let baseline = view.read_with(cx, |view, _| view.canonical);
        for (down, up, modifiers, expected) in [
            (
                at(0.4, 0.75),
                at(0.2, 0.75),
                Modifiers::default(),
                [0.2, 0.4],
            ),
            (at(0.3, 0.1), at(0.4, 0.1), Modifiers::default(), [0.2, 0.6]),
            (
                at(0.3, 0.1),
                at(0.6, 0.1),
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                [0.3, 0.6],
            ),
        ] {
            visual.simulate_mouse_down(down, MouseButton::Left, modifiers);
            visual.simulate_mouse_move(up, MouseButton::Left, modifiers);
            visual.simulate_mouse_up(up, MouseButton::Left, modifiers);
            for _ in 0..3 {
                view.update(cx, |_, cx| cx.notify());
                visual.run_until_parked();
                view.read_with(cx, |view, _| {
                    assert_eq!(view.canonical, baseline);
                    let actual = view.pending.unwrap()[0];
                    for i in 0..2 {
                        assert!(
                            (actual[i] - expected[i]).abs() < 0.001,
                            "{actual:?} != {expected:?}"
                        );
                    }
                });
            }
            visual.simulate_keystrokes("escape");
            view.update(cx, |view, cx| view.set_enabled(false, cx));
            view.read_with(cx, |view, _| assert!(view.pending.is_some()));
            view.update(cx, |view, cx| {
                view.settle(false, cx);
                view.set_enabled(true, cx);
            });
            visual.run_until_parked();
        }
        visual.simulate_mouse_down(at(0.4, 0.75), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.2, 0.75), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none());
            assert!(view.pending.is_none());
            assert_eq!(view.preview, baseline);
        });
        view.update(cx, |view, cx| view.set_enabled(false, cx));
        visual.simulate_mouse_down(at(0.3, 0.1), MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| assert!(view.capture.is_none()));
    }
    #[gpui::test]
    fn section_lower_body_drag_never_moves_section_geometry(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-body".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-body-rectangle-0").unwrap();
        let at = point(first.center().x, first.top() + first.size.height * 0.75);
        let before = view.read_with(cx, |view, _| view.canonical);
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(at.x + px(24.0), at.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        view.read_with(cx, |view, _| {
            assert_eq!(view.canonical, before);
            assert_eq!(
                view.preview, before,
                "lower body must not preview a section move"
            );
        });
    }

    #[gpui::test]
    fn section_body_range_commits_and_cancel_restores_previous_range(cx: &mut TestAppContext) {
        let handle = cx
            .add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-range-body".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-range-body-area").unwrap();
        let at = |fraction: f32| {
            point(
                area.left() + area.size.width * fraction,
                area.top() + area.size.height * 0.75,
            )
        };
        let before = view.read_with(cx, |view, _| view.canonical);
        visual.simulate_mouse_down(at(0.2), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.35), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at(0.35), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let committed = view.read_with(cx, |view, _| {
            assert_eq!(view.canonical, before);
            assert_eq!(view.preview, before);
            assert!(view.pending.is_none());
            view.range.expect("committed time range")
        });
        assert!(committed[1] > committed[0]);
        assert!(visual.debug_bounds("section-time-range").is_some());
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.4), MouseButton::Left, Modifiers::default());
        visual.simulate_keystrokes("escape");
        view.read_with(cx, |view, _| {
            assert_eq!(view.range, Some(committed));
            assert_eq!(view.canonical, before);
            assert!(view.capture.is_none());
        });
        visual.simulate_mouse_down(at(0.3), MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(at(0.4), MouseButton::Left, Modifiers::default());
        view.update(cx, |view, cx| assert!(view.finish(f32::NAN, cx)));
        view.read_with(cx, |view, _| {
            assert_eq!(view.range, Some(committed));
            assert!(view.capture.is_none());
        });
    }

    #[gpui::test]
    fn section_edges_and_extended_selection_win_over_lower_body_range(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-priority".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-priority-rectangle-0").unwrap();
        let edge = point(
            first.left() + px(2.0),
            first.top() + first.size.height * 0.75,
        );
        visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            let capture = view.capture.expect("edge capture");
            assert_eq!(capture.key, Target::Rectangle(0));
            assert_eq!(capture.part, RectanglePart::Leading);
            assert!(view.range.is_none());
        });
        visual.simulate_keystrokes("escape");
        let body = point(first.center().x, first.top() + first.size.height * 0.75);
        visual.simulate_mouse_down(
            body,
            MouseButton::Left,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        view.read_with(cx, |view, _| {
            assert!(view.capture.is_none());
            assert!(view.range.is_none());
            assert_eq!(view.selected, [0]);
        });
    }

    #[gpui::test]
    fn shift_click_same_section_keeps_only_that_section_and_never_starts_drag(
        cx: &mut TestAppContext,
    ) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-shift".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-shift-rectangle-0").unwrap();
        let at = point(first.center().x, first.top() + px(5.0));
        visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let other_before = visual.rendered_background("section-shift-rectangle-1");
        visual.simulate_mouse_down(
            at,
            MouseButton::Left,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(
                view.capture.is_none(),
                "shift selection must not start a drag"
            )
        });
        assert_eq!(
            visual.rendered_background("section-shift-rectangle-1"),
            other_before,
            "same-section range must not select the other section"
        );
    }

    #[gpui::test]
    fn extended_section_selection_keeps_anchor_and_never_captures_a_drag(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-range".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let first = visual.debug_bounds("section-range-rectangle-0").unwrap();
        let second = visual.debug_bounds("section-range-rectangle-1").unwrap();
        let a = point(first.center().x, first.top() + px(5.0));
        let b = point(second.center().x, second.top() + px(5.0));
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let toggle = Modifiers {
            shift: true,
            platform: true,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(a, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected, [0, 1]);
            assert_eq!(view.selection_anchor, Some(0));
            assert!(view.capture.is_none());
        });
        visual.simulate_mouse_up(b, MouseButton::Left, shift);
        visual.simulate_mouse_down(a, MouseButton::Left, toggle);
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected, [1]);
            assert_eq!(view.selection_primary, Some(1));
            assert_eq!(view.selection_anchor, Some(0));
            assert!(view.capture.is_none());
        });
        visual.simulate_mouse_up(a, MouseButton::Left, toggle);
        visual.simulate_mouse_down(b, MouseButton::Left, shift);
        view.read_with(cx, |view, _| {
            assert_eq!(view.selected, [1]);
            assert_eq!(view.selection_anchor, Some(1));
            assert!(view.capture.is_none());
        });
    }

    #[gpui::test]
    fn overlapping_section_edge_takes_priority_over_another_section_body(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "section-overlap".into(), cx));
        let view = handle.root(cx).unwrap();
        view.update(cx, |view, cx| {
            view.canonical = [[0.1, 0.8], [0.5, 0.9]];
            view.preview = view.canonical;
            cx.notify();
        });
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("section-overlap-area").unwrap();
        let edge = point(
            area.left() + area.size.width * 0.5 + px(2.0),
            area.top() + px(20.0),
        );
        visual.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, _| {
            let capture = view.capture.expect("section edge capture");
            assert_eq!(capture.key, Target::Rectangle(1));
            assert_eq!(capture.part, RectanglePart::Leading);
        });
    }

    #[gpui::test]
    fn rectangle_fixtures_hold_pending_across_repaints_cancel_and_refuse(cx: &mut TestAppContext) {
        for section in [false, true] {
            let handle = cx.add_window(|_, cx| {
                RectangleDemo::new(
                    if section { Kind::Section } else { Kind::Clip },
                    "rectangle-fixture".into(),
                    cx,
                )
            });
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
            if section {
                let pending = view.read_with(cx, |view, _| view.pending);
                let body = point(
                    released.center().x,
                    released.top() + released.size.height * 0.75,
                );
                visual.simulate_mouse_down(body, MouseButton::Left, Modifiers::default());
                view.read_with(cx, |view, _| {
                    assert_eq!(view.pending, pending);
                    assert!(view.capture.is_none());
                    assert!(view.range.is_none());
                });
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
                let handle = cx.add_window(|_, cx| {
                    RectangleDemo::new(
                        if section { Kind::Section } else { Kind::Clip },
                        "rectangle-narrow".into(),
                        cx,
                    )
                });
                let view = handle.root(cx).unwrap();
                let mut visual = VisualTestContext::from_window(*handle, cx);
                visual.update(|window, _| window.activate_window());
                visual.simulate_resize(gpui::size(px(240.0), px(240.0)));
                visual.simulate_mouse_move(point(px(1.0), px(1.0)), None, Modifiers::default());
                visual.run_until_parked();
                let a = visual.debug_bounds("rectangle-narrow-rectangle-0").unwrap();
                let b = visual.debug_bounds("rectangle-narrow-rectangle-1").unwrap();
                if section {
                    let header = visual
                        .debug_bounds("rectangle-narrow-section-header-0")
                        .unwrap();
                    assert!(
                        (f32::from(header.size.height) * 3.0 - f32::from(a.size.height)).abs()
                            <= 2.0
                    );
                    let (fill, _, _) =
                        theme::section_colors(hex_project::ElementColor::Cyan, false, false);
                    assert_eq!(
                        visual.rendered_background("rectangle-narrow-section-header-0"),
                        Some(gpui::rgb(fill).into())
                    );
                    assert_eq!(
                        visual.rendered_background("rectangle-narrow-rectangle-0"),
                        Some(gpui::rgba((fill << 8) | 0x38).into())
                    );
                }
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
                    let area = visual.debug_bounds("rectangle-narrow-area").unwrap();
                    assert!((b.left() - (area.left() + area.size.width * 0.7)).abs() <= px(0.25));
                    assert!((b.right() - (area.left() + area.size.width * 0.71)).abs() <= px(0.25));
                    assert!(
                        b.size.width < px(3.0),
                        "tiny clip is not widened for handles"
                    );
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
