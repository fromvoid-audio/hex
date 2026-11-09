//! Pointer capture and released depth display for native parameter assignments.
//! The project owns routes. A held pointer edits a local depth; one bank
//! replacement on release takes display custody through its worker receipt.
use super::{DEFAULT_MAPPING_DEPTH, DevicePanelOwner, Edit, MappingOutput, MappingTarget, apply};
use crate::editor::{
    EditorMode, EditorView,
    pending_edit::{Disposition, Receipt},
};
use crate::theme;
use gpui::{
    AnyElement, Context, CursorStyle, ElementId, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Styled, Window, canvas, div, px, rgb,
};
use hex_lattice_contract::ParameterId;
use hex_project::{
    DeviceId, DeviceModulation, DeviceModulationRoute, ModulationAmount, ModulationRouteId,
};
use hex_session::service::{Event, Request};

const CURSOR: CursorStyle = CursorStyle::ResizeUpDown;
const DRAG_THRESHOLD: f32 = 4.0;
const PIXELS_PER_UNIT: f32 = 100.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Target {
    pub(super) device: DeviceId,
    pub(super) source: MappingOutput,
    pub(super) parameter: ParameterId,
}

#[derive(Clone, Copy)]
struct Origin {
    target: Target,
    revision: u64,
    generation: u64,
    owner: Option<DevicePanelOwner>,
    mode: EditorMode,
}

impl Origin {
    fn current(self, view: &EditorView) -> bool {
        view.clip_view_open
            && view.device_panel_active
            && self.generation == view.project_generation
            && self.owner == view.device_panel.owner
            && self.mode == view.mode
            && view.device_panel.mapping_source == Some((self.target.device, self.target.source))
            && view
                .state
                .as_ref()
                .is_some_and(|state| state.revision == self.revision)
    }
}

struct Gesture {
    origin: Origin,
    route: Option<ModulationRouteId>,
    start: Point<Pixels>,
    start_depth: f32,
    depth: f32,
    dragged: bool,
    focus: Option<FocusHandle>,
}

struct Pending {
    origin: Origin,
    depth: f32,
    bank: DeviceModulation,
    receipt: Receipt,
}

impl Pending {
    fn matches(&self, request: &Request) -> bool {
        self.receipt.is_queued()
            && matches!(
                request,
                Request::ReplaceDeviceModulation {
                    project_generation,
                    revision,
                    device,
                    modulation,
                } if *project_generation == self.origin.generation
                    && *revision == self.origin.revision
                    && *device == self.origin.target.device
                    && modulation == &self.bank
            )
    }
}

#[derive(Default)]
pub(in crate::editor::device_panel) struct State {
    gesture: Option<Gesture>,
    pending: Option<Pending>,
}

impl State {
    fn displayed_depth(&self, view: &EditorView, target: Target) -> Option<f32> {
        self.gesture
            .as_ref()
            .filter(|g| g.origin.target == target && g.origin.current(view) && g.dragged)
            .map(|g| g.depth)
            .or_else(|| {
                self.pending
                    .as_ref()
                    .filter(|p| p.origin.target == target)
                    .map(|p| p.depth)
            })
    }
}

pub(super) fn overlay(
    view: &EditorView,
    cx: &mut Context<EditorView>,
    target: Target,
    route: Option<&DeviceModulationRoute>,
    revision: u64,
) -> AnyElement {
    let origin = Origin {
        target,
        revision,
        generation: view.project_generation,
        owner: view.device_panel.owner,
        mode: view.mode,
    };
    let route = route.map(|route| (route.id, route.amount.get()));
    let displayed = view
        .device_panel
        .modulation_target_drag
        .displayed_depth(view, target)
        .or_else(|| route.map(|(_, depth)| depth));
    let selector = format!(
        "map-target-{}-{}",
        target.device.value(),
        target.parameter.value()
    );
    let entity = cx.entity();
    div()
        .id(ElementId::Name(selector.clone().into()))
        .debug_selector(move || selector.clone())
        .absolute()
        .inset_0()
        .border_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Primary)))
        .cursor(CURSOR)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                begin(view, event, window, cx, origin, route);
            }),
        )
        .children(displayed.map(|depth| {
            let label = format!("{:+.0}%", depth * 100.0);
            let selector = format!(
                "map-depth-{}-{}-{label}",
                target.device.value(),
                target.parameter.value()
            );
            div()
                .absolute()
                .top_0()
                .right_0()
                .px(px(2.0))
                .text_size(px(9.0))
                .bg(rgb(theme::color(theme::PaletteRole::Background)))
                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                .debug_selector(move || selector.clone())
                .child(theme::text(label))
        }))
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, cx| {
                    let view = entity.read(cx);
                    if view
                        .device_panel
                        .modulation_target_drag
                        .gesture
                        .as_ref()
                        .is_some_and(|g| {
                            g.origin.target == target
                                && g.origin.current(view)
                                && g.focus == window.focused(cx)
                        })
                    {
                        window.renew_pointer_capture(entity.entity_id(), CURSOR);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .into_any_element()
}

fn begin(
    view: &mut EditorView,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<EditorView>,
    origin: Origin,
    route: Option<(ModulationRouteId, f32)>,
) {
    if !f32::from(event.position.x).is_finite()
        || !f32::from(event.position.y).is_finite()
        || !origin.current(view)
        || view.edit_pending()
        || view.device_panel.modulation_target_drag.gesture.is_some()
        || view.device_panel.modulation_target_drag.pending.is_some()
    {
        return;
    }
    let entity = cx.entity().downgrade();
    let cancelled = entity.clone();
    if !window.capture_pointer(
        cx.entity_id(),
        MouseButton::Left,
        CURSOR,
        move |input, window, cx| {
            let released = input.downcast_ref::<MouseUpEvent>();
            let position = input
                .downcast_ref::<MouseMoveEvent>()
                .map(|event| event.position)
                .or_else(|| released.map(|event| event.position));
            if let Some(position) = position {
                let _ = entity.update(cx, |view, cx| {
                    update(view, window, cx, position, released.is_some());
                    cx.notify();
                });
            }
        },
        move |_, cx| {
            let _ = cancelled.update(cx, |view, cx| {
                if view.cancel_modulation_target_drag() {
                    cx.notify();
                }
            });
        },
        cx,
    ) {
        return;
    }
    let start_depth = route.map_or(0.0, |(_, depth)| depth);
    view.device_panel.modulation_target_drag.gesture = Some(Gesture {
        origin,
        route: route.map(|(id, _)| id),
        start: event.position,
        start_depth,
        depth: start_depth,
        dragged: false,
        focus: window.focused(cx),
    });
    cx.notify();
}

fn update(
    view: &mut EditorView,
    window: &Window,
    cx: &Context<EditorView>,
    position: Point<Pixels>,
    released: bool,
) {
    let Some(mut gesture) = view.device_panel.modulation_target_drag.gesture.take() else {
        return;
    };
    let delta = f32::from(gesture.start.y - position.y);
    let sideways = f32::from(gesture.start.x - position.x).abs();
    if !gesture.origin.current(view)
        || gesture.focus != window.focused(cx)
        || !delta.is_finite()
        || !sideways.is_finite()
    {
        return;
    }
    gesture.dragged |= delta.abs() >= DRAG_THRESHOLD;
    if gesture.dragged {
        gesture.depth = (gesture.start_depth + delta / PIXELS_PER_UNIT).clamp(-1.0, 1.0);
    }
    if !released {
        view.device_panel.modulation_target_drag.gesture = Some(gesture);
        return;
    }
    let origin = gesture.origin;
    let target = origin.target;
    let (edit, depth) = match gesture.route {
        Some(id) if gesture.depth != gesture.start_depth => {
            let Ok(depth) = ModulationAmount::new(gesture.depth) else {
                return;
            };
            (Edit::SetDepth(id, depth), gesture.depth)
        }
        Some(_) => return,
        None if !gesture.dragged && sideways >= DRAG_THRESHOLD => return,
        None => {
            let depth = if gesture.dragged {
                gesture.depth
            } else {
                DEFAULT_MAPPING_DEPTH
            };
            (
                Edit::Map(
                    target.source,
                    MappingTarget::Device(target.parameter),
                    depth,
                ),
                depth,
            )
        }
    };
    let Some(state) = view.state.as_ref() else {
        return;
    };
    let bank = match apply(&state.project, target.device, edit) {
        Ok(bank) => bank,
        Err(error) => {
            view.error = Some(error.0.into());
            return;
        }
    };
    view.device_panel.modulation_target_drag.pending = Some(Pending {
        origin,
        depth,
        bank: bank.clone(),
        receipt: Receipt::new(origin.generation),
    });
    if !view.enqueue(Request::ReplaceDeviceModulation {
        project_generation: origin.generation,
        revision: origin.revision,
        device: target.device,
        modulation: bank,
    }) {
        view.retire_pending_modulation_depth();
    }
}

impl EditorView {
    pub(in crate::editor) fn cancel_modulation_target_drag(&mut self) -> bool {
        self.device_panel
            .modulation_target_drag
            .gesture
            .take()
            .is_some()
    }

    pub(in crate::editor) fn pending_modulation_depth_matches(&self, request: &Request) -> bool {
        self.device_panel
            .modulation_target_drag
            .pending
            .as_ref()
            .is_some_and(|p| p.matches(request))
    }

    pub(in crate::editor) fn modulation_depth_submitted(&mut self) {
        if let Some(pending) = &mut self.device_panel.modulation_target_drag.pending {
            pending.receipt.submitted(self.worker_requests_pending);
        }
    }

    pub(in crate::editor) fn retire_pending_modulation_depth(&mut self) -> bool {
        self.device_panel
            .modulation_target_drag
            .pending
            .take()
            .is_some()
    }

    pub(in crate::editor) fn retire_abandoned_modulation_depth(&mut self) -> bool {
        if self
            .device_panel
            .modulation_target_drag
            .pending
            .as_ref()
            .is_some_and(|pending| {
                pending.receipt.is_queued()
                    && !self
                        .pending_requests
                        .iter()
                        .chain(self.device_gesture_barriers.iter())
                        .any(|request| pending.matches(request))
            })
        {
            return self.retire_pending_modulation_depth();
        }
        false
    }

    pub(in crate::editor) fn modulation_depth_complete(&mut self, event: &Event) {
        if self
            .device_panel
            .modulation_target_drag
            .pending
            .as_mut()
            .is_some_and(|p| {
                p.receipt.observe(
                    self.project_generation,
                    event.project_replaced,
                    event.request_complete,
                ) != Disposition::Waiting
            })
        {
            self.retire_pending_modulation_depth();
        }
    }
}
