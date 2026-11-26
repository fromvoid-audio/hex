//! Disposable rectangle, section-surface, and time-range widget fixtures.
//! Exercises shared `hex-widgets` capture/surfaces and `hex-ui-kit` section
//! surfaces without duplicating `hex-ui`'s full arrangement section controller.
#[path = "section_fixture.rs"]
mod section_fixture;

use gpui::{
    Bounds, Context, FocusHandle, Focusable, Modifiers, MouseButton, Pixels, Render, Subscription,
    Window, div, prelude::*, px, rgb,
};
use hex_project::{ArrangementSection, Command};
use hex_ui_kit::{section_hit, section_surface, theme};
use hex_ui_model::pending_edit;
use hex_widgets::{
    RangeSelectionTarget, RangeSelectionZones, RectangleCapture, RectangleEdges, RectanglePart,
    rectangle_body, rectangle_edge, rectangle_surface,
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
}

enum Pending {
    Local {
        display: Vec<[f32; 2]>,
    },
    Section {
        display: Vec<[f32; 2]>,
        sections: Vec<ArrangementSection>,
        command: Box<Command>,
        receipt: pending_edit::Receipt,
    },
}

impl Pending {
    fn display(&self) -> Vec<[f32; 2]> {
        match self {
            Self::Local { display } | Self::Section { display, .. } => display.clone(),
        }
    }
}

pub(super) struct RectangleDemo {
    kind: Kind,
    id: String,
    focus: FocusHandle,
    blur: Option<Subscription>,
    bounds: Option<Bounds<Pixels>>,
    canonical: Vec<[f32; 2]>,
    preview: Vec<[f32; 2]>,
    capture: Option<RectangleCapture<Target>>,
    pending: Option<Pending>,
    section_fixture: Option<section_fixture::Fixture>,
    section_preview: Vec<ArrangementSection>,
    section_command: Option<Command>,
    section_focus: bool,
    generation: u64,
    selected: Vec<usize>,
    enabled: bool,
    status: &'static str,
}

impl RectangleDemo {
    pub fn new(kind: Kind, id: String, cx: &Context<Self>) -> Self {
        let (section_fixture, initial_error) = if kind == Kind::Section {
            match section_fixture::Fixture::new() {
                Ok(fixture) => (Some(fixture), None),
                Err(error) => (None, Some(error.0)),
            }
        } else {
            (None, None)
        };
        let canonical = if kind == Kind::Section {
            section_fixture
                .as_ref()
                .map_or_else(Vec::new, section_fixture::Fixture::display)
        } else {
            vec![[0.1, 0.5], [0.7, 0.71]]
        };
        let section_preview = section_fixture.as_ref().map_or_else(Vec::new, |fixture| {
            fixture.project().arrangement_sections().to_vec()
        });
        Self {
            kind,
            id,
            focus: cx.focus_handle(),
            blur: None,
            bounds: None,
            canonical: canonical.clone(),
            preview: canonical,
            capture: None,
            pending: None,
            section_fixture,
            section_preview,
            section_command: None,
            section_focus: false,
            generation: 1,
            selected: if kind == Kind::TimeRange {
                vec![0]
            } else {
                Vec::new()
            },
            enabled: true,
            status: initial_error.unwrap_or("ready"),
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
            let _ = capture.cancel();
            self.preview = self.canonical.clone();
            if let Some(fixture) = self.section_fixture.as_ref() {
                self.section_preview = fixture.project().arrangement_sections().to_vec();
            }
            self.section_command = None;
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
        if !self.enabled
            || self.pending.is_some()
            || (self.kind == Kind::Section && self.section_fixture.is_none())
        {
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
            self.preview = self.canonical.clone();
            self.selected = vec![0];
            self.status = "held range preview";
            self.focus.focus(window);
            cx.notify();
            return true;
        }
        let mut hit = if self.kind == Kind::Section {
            self.section_fixture.as_ref().and_then(|fixture| {
                let sections = fixture
                    .project()
                    .arrangement_sections()
                    .iter()
                    .enumerate()
                    .map(|(index, section)| (index, section.start(), section.end()));
                section_hit::hit(sections, x, |beat| {
                    Some(left + width * fixture.position(beat))
                })
                .map(|hit| match hit {
                    section_hit::Hit::Leading(index) => (index, RectanglePart::Leading),
                    section_hit::Hit::Trailing(index) => (index, RectanglePart::Trailing),
                    section_hit::Hit::Seam(left, _) => (left, RectanglePart::Seam),
                })
            })
        } else {
            None
        };
        if hit.is_none() {
            for (index, [start, end]) in self.canonical.iter().copied().enumerate() {
                if let Some(part) = self
                    .edges()
                    .hit(x, left + start * width, left + end * width)
                {
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
        let Some((index, part)) = hit else {
            return false;
        };
        self.selected = vec![index];
        self.capture = RectangleCapture::begin(Target::Rectangle(index), part, x, true);
        self.preview = self.canonical.clone();
        if let Some(fixture) = self.section_fixture.as_ref() {
            self.section_preview = fixture.project().arrangement_sections().to_vec();
        }
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
        let left = f32::from(bounds.left());
        let width = f32::from(bounds.size.width);
        if !width.is_finite() || width <= 0.0 {
            return true;
        }
        if intent.key == Target::PaintRange {
            let anchor = ((intent.origin_x - left) / width).clamp(0.0, 1.0);
            let end = ((intent.pointer_x - left) / width).clamp(0.0, 1.0);
            self.preview[0] = [anchor.min(end), anchor.max(end)];
            cx.notify();
            return true;
        }
        let Target::Rectangle(index) = intent.key else {
            return true;
        };
        if self.kind == Kind::Section {
            self.section_command = None;
            self.preview = self.canonical.clone();
            if let Some(fixture) = self.section_fixture.as_ref() {
                self.section_preview = fixture.project().arrangement_sections().to_vec();
                if (intent.pointer_x - intent.origin_x).abs() >= 4.0
                    && let Some(section) = fixture.project().arrangement_sections().get(index)
                {
                    let plan = fixture
                        .snapped_beat(intent.pointer_x, left, width, false)
                        .and_then(|beat| {
                            fixture.plan_at_beat(section.id(), intent.part, beat, false, false)
                        });
                    match plan {
                        Ok(Some(plan)) => {
                            self.preview = plan.display;
                            self.section_preview = plan.sections;
                            self.section_command = Some(plan.command);
                        }
                        Ok(None) => {}
                        Err(error) => self.status = error.0,
                    }
                }
            }
            cx.notify();
            return true;
        }
        let delta = intent.delta_x / width;
        let [start, end] = self.canonical[index];
        let minimum = (1.0 / width).min(end - start);
        self.preview = self.canonical.clone();
        match intent.part {
            RectanglePart::Body => {
                let delta = delta.clamp(-start, 1.0 - end);
                self.preview[index] = [start + delta, end + delta];
            }
            RectanglePart::Leading => {
                self.preview[index][0] = (start + delta).clamp(0.0, end - minimum);
            }
            RectanglePart::Trailing => {
                self.preview[index][1] = (end + delta).clamp(start + minimum, 1.0);
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
        let finished = self
            .capture
            .take()
            .and_then(|capture| capture.finish(x))
            .is_some();
        if finished
            && if self.kind == Kind::Section {
                self.section_command.is_some()
            } else {
                self.preview != self.canonical
            }
        {
            if self.kind == Kind::Section {
                let mut receipt = pending_edit::Receipt::new(self.generation);
                if receipt.submitted(1)
                    && let Some(command) = self.section_command.take()
                {
                    self.pending = Some(Pending::Section {
                        display: self.preview.clone(),
                        sections: self.section_preview.clone(),
                        command: Box::new(command),
                        receipt,
                    });
                    self.status = "pending · enter accepts · backspace refuses";
                } else {
                    self.preview = self.canonical.clone();
                    self.status = "section edit refused";
                }
            } else {
                self.pending = Some(Pending::Local {
                    display: self.preview.clone(),
                });
                self.status = "pending · enter accepts · backspace refuses";
            }
        } else {
            self.preview = self.canonical.clone();
            self.status = "ready";
        }
        cx.notify();
        true
    }

    fn settle(&mut self, accept: bool, cx: &mut Context<Self>) {
        match self.pending.take() {
            Some(Pending::Section {
                command,
                mut receipt,
                ..
            }) => {
                let disposition = receipt.observe(self.generation, false, true);
                if disposition == pending_edit::Disposition::Terminal && accept {
                    if let Some(fixture) = self.section_fixture.as_mut() {
                        match fixture.commit(*command) {
                            Ok(_) => {
                                self.canonical = fixture.display();
                                self.section_preview =
                                    fixture.project().arrangement_sections().to_vec();
                                self.status = "accepted";
                            }
                            Err(error) => self.status = error.0,
                        }
                    }
                } else {
                    if let Some(fixture) = self.section_fixture.as_ref() {
                        self.section_preview = fixture.project().arrangement_sections().to_vec();
                    }
                    self.status = "refused";
                }
                self.preview = self.canonical.clone();
                cx.notify();
            }
            Some(Pending::Local { display }) => {
                if accept {
                    self.canonical = display;
                }
                self.preview = self.canonical.clone();
                self.status = if accept { "accepted" } else { "refused" };
                cx.notify();
            }
            None => {}
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
        let display = self
            .pending
            .as_ref()
            .map(Pending::display)
            .unwrap_or_else(|| {
                if self.capture.is_some() {
                    self.preview.clone()
                } else {
                    self.canonical.clone()
                }
            });
        let section_paint = match self.pending.as_ref() {
            Some(Pending::Section { sections, .. }) => sections.as_slice(),
            _ if self.capture.is_some() => self.section_preview.as_slice(),
            _ => self
                .section_fixture
                .as_ref()
                .map_or(&[][..], |fixture| fixture.project().arrangement_sections()),
        };
        let width = self
            .bounds
            .map_or(320.0_f32, |bounds| f32::from(bounds.size.width));
        let mut area = div()
            .id(gpui::SharedString::from(format!("{}-area", self.id)))
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-area")
            })
            .relative()
            .w_full()
            .h(px(match self.kind {
                Kind::Section => 48.0,
                Kind::TimeRange => 56.0,
                Kind::Clip => 72.0,
            }))
            .bg(rgb(style.rail))
            .border_1()
            .border_color(rgb(style.border));

        for (index, [start, end]) in display.iter().copied().enumerate() {
            if self.kind == Kind::TimeRange && index > 0 {
                continue;
            }
            let selected = self.selected.contains(&index);
            let pixel_width = width * (end - start);
            let surface = if self.kind == Kind::Section {
                let Some(section) = section_paint.get(index) else {
                    continue;
                };
                let identity = section.color();
                section_surface::surface(
                    (start, end - start),
                    pixel_width,
                    section_surface::CHAMFER,
                    identity,
                    selected,
                    false,
                    None,
                )
                .cursor(RectanglePart::Body.cursor())
                .children((self.section_focus && selected).then(|| {
                    section_surface::focus::overlay(identity, section_surface::CHAMFER)
                        .debug_selector({
                            let id = self.id.clone();
                            move || format!("{id}-section-focus-{index}")
                        })
                }))
                .child(
                    section_surface::header(identity, selected, false, 14.0)
                        .debug_selector({
                            let id = self.id.clone();
                            move || format!("{id}-section-header-{index}")
                        })
                        .child(theme::text(section.name())),
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
                .opacity(if !self.enabled {
                    0.4
                } else if self.pending.is_some() {
                    0.85
                } else {
                    1.0
                });
            let border = hex_widgets::rectangle_border_width(pixel_width);
            let handle = self.edges().handle_width(pixel_width);
            if self.kind == Kind::TimeRange
                && let Some(zones) = RangeSelectionZones::new(56.0, 0.0)
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

        let entity = cx.entity();
        area = area.child(
            gpui::canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    entity.update(cx, |view, _| view.bounds = Some(bounds));
                    let down = entity.clone();
                    window.on_mouse_event(
                        move |event: &gpui::MouseDownEvent, phase, window, cx| {
                            if phase.bubble()
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
                    "q" if view.kind == Kind::Section && !event.keystroke.modifiers.modified() => {
                        if !event.is_held {
                            view.section_focus = !view.section_focus;
                            cx.notify();
                        }
                    }
                    "escape" => view.cancel(cx),
                    "enter" => view.settle(true, cx),
                    "backspace" => view.settle(false, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(area)
            .child(theme::text(self.status))
            .children(
                (self.kind == Kind::Section).then(|| theme::text("q toggles section focus paint")),
            )
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
    fn q_previews_shared_section_focus_without_editing_the_project(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::Section, "focus-fixture".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.selected = vec![0];
                v.focus.focus(window);
                cx.notify();
            })
        });
        visual.run_until_parked();
        let before = view.read_with(cx, |v, _| {
            v.section_fixture.as_ref().unwrap().project().snapshot()
        });
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-0")
                .is_none()
        );
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-0")
                .is_some()
        );
        visual.simulate_keystrokes("q");
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("focus-fixture-section-focus-0")
                .is_none()
        );
        view.read_with(cx, |v, _| {
            assert_eq!(
                v.section_fixture.as_ref().unwrap().project().snapshot(),
                before
            )
        });
    }

    #[gpui::test]
    fn clip_and_time_range_pending_edits_accept_and_refuse_cleanly(cx: &mut TestAppContext) {
        let handle =
            cx.add_window(|_, cx| RectangleDemo::new(Kind::TimeRange, "range-fixture".into(), cx));
        let view = handle.root(cx).unwrap();
        let mut visual = VisualTestContext::from_window(*handle, cx);
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
        let area = visual.debug_bounds("range-fixture-area").unwrap();
        let before = view.read_with(cx, |v, _| v.canonical[0]);
        let start = point(area.left() + area.size.width * 0.25, area.top() + px(8.0));
        let end = point(area.left() + area.size.width * 0.45, area.top() + px(8.0));
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        view.read_with(cx, |v, _| assert!(v.pending.is_some()));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        view.read_with(cx, |v, _| {
            assert!(v.pending.is_none());
            assert_ne!(v.canonical[0], before);
            assert_eq!(v.status, "accepted");
        });
    }
}
